use crate::config::{EngineId, TranslationConfig, is_loopback_url};
use crate::languages::{
    deepl_supports_formality, normalize_detected_code, to_alibaba_code, to_baidu_code,
    to_deepl_source, to_deepl_target, to_google_code, to_microsoft_code, to_openai_name,
    to_youdao_code,
};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use md5::Md5;
use openidea_plugin::{Error, HttpRequest, host};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NONCE_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Phonetics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub us: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub general: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DictEntry {
    pub pos: String,
    pub terms: Vec<String>,
    #[serde(default)]
    pub explains: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExampleItem {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationResult {
    pub engine: String,
    pub original_text: String,
    pub query_text: String,
    pub translation: String,
    pub source_lang: String,
    pub target_lang: String,
    #[serde(default)]
    pub phonetics: Phonetics,
    #[serde(default)]
    pub dict: Vec<DictEntry>,
    #[serde(default)]
    pub synonyms: Vec<String>,
    #[serde(default)]
    pub examples: Vec<ExampleItem>,
    #[serde(default)]
    pub alternatives: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spell_correction: Option<String>,
    pub from_cache: bool,
    pub latency_ms: u64,
}

#[derive(Clone, Debug)]
pub struct EdgeToken {
    pub token: String,
    pub expires_at_secs: u64,
}

pub fn unix_secs_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn unix_millis_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + if m <= 2 { 1 } else { 0 }, m as u32, d as u32)
}

pub fn format_utc_iso8601(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let h = rem / 3_600;
    let m = (rem % 3_600) / 60;
    let s = rem % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

pub fn next_nonce() -> String {
    let seq = NONCE_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}-{seq}", unix_millis_now())
}

/// Strict RFC 3986 percent-encoding (spaces encoded as `%20`, `A-Za-z0-9-_.~` unescaped).
pub fn percent_encode_rfc3986(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for b in input.bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_upper(b >> 4));
            out.push(hex_upper(b & 0x0F));
        }
    }
    out
}

fn hex_upper(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'A' + (nibble - 10)) as char,
    }
}

pub fn form_urlencode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode_rfc3986(k), percent_encode_rfc3986(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Youdao v3 `input` truncation rule based on Unicode character count.
pub fn youdao_truncate(q: &str) -> String {
    let count = q.chars().count();
    if count <= 20 {
        return q.to_owned();
    }
    let head: String = q.chars().take(10).collect();
    let tail: String = q.chars().skip(count - 10).collect();
    format!("{head}{count}{tail}")
}

pub fn youdao_v3_sign(
    app_key: &str,
    q: &str,
    salt: &str,
    curtime: &str,
    app_secret: &str,
) -> String {
    let input = youdao_truncate(q);
    let raw = format!("{app_key}{input}{salt}{curtime}{app_secret}");
    hex::encode(Sha256::digest(raw.as_bytes()))
}

pub fn baidu_md5_sign(app_id: &str, q: &str, salt: &str, secret_key: &str) -> String {
    let raw = format!("{app_id}{q}{salt}{secret_key}");
    hex::encode(Md5::digest(raw.as_bytes()))
}

pub fn alibaba_rpc_sign(
    method: &str,
    params: &BTreeMap<String, String>,
    access_key_secret: &str,
) -> Result<String, Error> {
    let canonical_query = params
        .iter()
        .filter(|(k, _)| k.as_str() != "Signature")
        .map(|(k, v)| format!("{}={}", percent_encode_rfc3986(k), percent_encode_rfc3986(v)))
        .collect::<Vec<_>>()
        .join("&");
    let string_to_sign = format!(
        "{}&{}&{}",
        method.to_ascii_uppercase(),
        percent_encode_rfc3986("/"),
        percent_encode_rfc3986(&canonical_query)
    );
    let signing_key = format!("{access_key_secret}&");
    let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(signing_key.as_bytes())
        .map_err(|e| Error::Failed(format!("Alibaba HMAC init error: {e}")))?;
    mac.update(string_to_sign.as_bytes());
    Ok(STANDARD.encode(mac.finalize().into_bytes()))
}

pub fn translate_with_engine(
    engine: EngineId,
    raw_text: &str,
    query_text: &str,
    source_lang: &str,
    target_lang: &str,
    cfg: &TranslationConfig,
    edge_token: &mut Option<EdgeToken>,
) -> Result<TranslationResult, Error> {
    translate_with_engine_mode(
        engine,
        raw_text,
        query_text,
        source_lang,
        target_lang,
        cfg,
        edge_token,
        false,
    )
}

pub fn translate_with_engine_mode(
    engine: EngineId,
    raw_text: &str,
    query_text: &str,
    source_lang: &str,
    target_lang: &str,
    cfg: &TranslationConfig,
    edge_token: &mut Option<EdgeToken>,
    is_document: bool,
) -> Result<TranslationResult, Error> {
    let start_ms = unix_millis_now();
    let mut res = match engine {
        EngineId::Google => translate_google(query_text, source_lang, target_lang, cfg)?,
        EngineId::Microsoft => {
            translate_microsoft(query_text, source_lang, target_lang, cfg, edge_token)?
        }
        EngineId::Openai => {
            translate_openai(query_text, source_lang, target_lang, cfg, is_document)?
        }
        EngineId::Deepl => translate_deepl(query_text, source_lang, target_lang, cfg)?,
        EngineId::Youdao => translate_youdao(query_text, source_lang, target_lang, cfg)?,
        EngineId::Baidu => translate_baidu(query_text, source_lang, target_lang, cfg)?,
        EngineId::Alibaba => translate_alibaba(query_text, source_lang, target_lang, cfg)?,
    };
    res.engine = engine.as_str().to_owned();
    res.original_text = raw_text.to_owned();
    res.query_text = query_text.to_owned();
    res.from_cache = false;
    res.latency_ms = unix_millis_now().saturating_sub(start_ms);
    Ok(res)
}

fn truncate_msg(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push_str("...");
    }
    out
}

fn check_http_status(status: u16, body: &str, engine: &str) -> Result<(), Error> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    Err(Error::Failed(format!(
        "{engine} HTTP {status}: {}",
        truncate_msg(body.trim(), 240)
    )))
}

// ---------------------------------------------------------------------------
// 1. Google Translate (Free gtx + Cloud API v2)
// ---------------------------------------------------------------------------

fn translate_google(
    text: &str,
    source_lang: &str,
    target_lang: &str,
    cfg: &TranslationConfig,
) -> Result<TranslationResult, Error> {
    if cfg.google_mode == "cloud-api" {
        return translate_google_cloud(text, source_lang, target_lang, cfg);
    }

    let host = if cfg.google_custom_host.is_empty() {
        "translate.googleapis.com"
    } else {
        cfg.google_custom_host
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
    };
    let sl = to_google_code(source_lang);
    let tl = to_google_code(target_lang);
    let hl = if cfg.is_zh() { "zh-CN" } else { "en" };

    let extra_dt = if cfg.google_use_spell_check {
        "&dt=at&dt=qca"
    } else {
        "&dt=at"
    };
    let url = format!(
        "https://{host}/translate_a/single?client=gtx&sl={}&tl={}&hl={}&dt=t&dt=bd&dt=md&dt=rm&dt=ss&dt=ex{extra_dt}&dj=1&q={}",
        percent_encode_rfc3986(sl),
        percent_encode_rfc3986(tl),
        percent_encode_rfc3986(hl),
        percent_encode_rfc3986(text)
    );
    let mut req = HttpRequest::get(&url).header("Accept", "application/json");
    req.timeout_ms = Some(15_000);
    let resp = host::fetch(&req)?;
    let body = resp.text();
    check_http_status(resp.status, &body, "Google Translate")?;

    let v: Value = serde_json::from_str(&body)
        .map_err(|e| Error::Failed(format!("Google response parse error: {e}")))?;

    let mut translated = String::new();
    let mut src_translit: Option<String> = None;
    let mut tgt_translit: Option<String> = None;
    if let Some(sentences) = v.get("sentences").and_then(Value::as_array) {
        for s in sentences {
            if let Some(t) = s.get("trans").and_then(Value::as_str) {
                translated.push_str(t);
            }
            if src_translit.is_none()
                && let Some(p) = s.get("src_translit").and_then(Value::as_str)
                && !p.trim().is_empty()
            {
                src_translit = Some(p.trim().to_owned());
            }
            if tgt_translit.is_none()
                && let Some(p) = s.get("translit").and_then(Value::as_str)
                && !p.trim().is_empty()
            {
                tgt_translit = Some(p.trim().to_owned());
            }
        }
    }
    if translated.trim().is_empty() {
        return Err(Error::Failed("Google returned an empty translation".into()));
    }

    let mut dict = Vec::new();
    let mut alternatives = Vec::new();
    if let Some(entries) = v.get("dict").and_then(Value::as_array) {
        for entry in entries {
            let pos = entry
                .get("pos")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned();
            let terms: Vec<String> = entry
                .get("terms")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            for term in terms.iter().take(4) {
                if !alternatives.contains(term) && term != translated.trim() {
                    alternatives.push(term.clone());
                }
            }
            let mut explains = Vec::new();
            if let Some(detail_arr) = entry.get("entry").and_then(Value::as_array) {
                for item in detail_arr.iter().take(5) {
                    if let Some(word) = item.get("word").and_then(Value::as_str) {
                        let rev: Vec<&str> = item
                            .get("reverse_translation")
                            .and_then(Value::as_array)
                            .map(|r| r.iter().filter_map(Value::as_str).take(3).collect())
                            .unwrap_or_default();
                        if rev.is_empty() {
                            explains.push(word.to_owned());
                        } else {
                            explains.push(format!("{word} ({})", rev.join(", ")));
                        }
                    }
                }
            }
            if !terms.is_empty() || !explains.is_empty() {
                dict.push(DictEntry { pos, terms, explains });
            }
        }
    }

    let mut synonyms = Vec::new();
    if let Some(synsets) = v.get("synsets").and_then(Value::as_array) {
        for syn in synsets {
            if let Some(entries) = syn.get("entry").and_then(Value::as_array) {
                for group in entries {
                    if let Some(list) = group.get("synonym").and_then(Value::as_array) {
                        for w in list.iter().filter_map(Value::as_str) {
                            if !synonyms.contains(&w.to_owned()) && synonyms.len() < 12 {
                                synonyms.push(w.to_owned());
                            }
                        }
                    }
                }
            }
        }
    }

    let mut examples = Vec::new();
    if let Some(exs) = v
        .get("examples")
        .and_then(|e| e.get("example"))
        .and_then(Value::as_array)
    {
        for ex in exs.iter().take(5) {
            if let Some(raw_ex) = ex.get("text").and_then(Value::as_str) {
                let clean = strip_html_tags(raw_ex);
                if !clean.is_empty() {
                    examples.push(ExampleItem {
                        source: clean,
                        target: None,
                    });
                }
            }
        }
    }

    if let Some(alt_blocks) = v.get("alternative_translations").and_then(Value::as_array) {
        for block in alt_blocks {
            if let Some(alt_arr) = block.get("alternative").and_then(Value::as_array) {
                for item in alt_arr.iter().take(6) {
                    if let Some(cand) = item.get("word_postproc").and_then(Value::as_str) {
                        let clean = cand.trim().to_owned();
                        if !clean.is_empty()
                            && clean != translated.trim()
                            && !alternatives.contains(&clean)
                            && alternatives.len() < 10
                        {
                            alternatives.push(clean);
                        }
                    }
                }
            }
        }
    }

    let spell_correction = if cfg.google_use_spell_check {
        v.get("spell").and_then(|sp| {
            sp.get("spell_res")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    sp.get("spell_html_res")
                        .and_then(Value::as_str)
                        .map(strip_html_tags)
                        .filter(|s| !s.is_empty())
                })
        })
    } else {
        None
    };

    let detected = v
        .get("src")
        .and_then(Value::as_str)
        .map(normalize_detected_code)
        .unwrap_or_else(|| source_lang.to_owned());

    Ok(TranslationResult {
        translation: translated.trim().to_owned(),
        source_lang: if source_lang == "auto" {
            detected
        } else {
            source_lang.to_owned()
        },
        target_lang: target_lang.to_owned(),
        phonetics: Phonetics {
            general: src_translit.or(tgt_translit),
            ..Phonetics::default()
        },
        dict,
        synonyms,
        examples,
        alternatives,
        spell_correction,
        ..TranslationResult::default()
    })
}

fn translate_google_cloud(
    text: &str,
    source_lang: &str,
    target_lang: &str,
    _cfg: &TranslationConfig,
) -> Result<TranslationResult, Error> {
    if !host::secret_is_set("google-api-key") {
        return Err(Error::Invalid(
            "Google Cloud API mode requires secret `google-api-key` in Settings -> Plugins".into(),
        ));
    }
    let mut payload = json!({
        "q": text,
        "target": to_google_code(target_lang),
        "format": "text"
    });
    if source_lang != "auto" {
        payload["source"] = Value::String(to_google_code(source_lang).to_owned());
    }
    let mut req = HttpRequest::post(
        "https://translation.googleapis.com/language/translate/v2",
        payload.to_string(),
    )
    .header("Content-Type", "application/json")
    .header("X-Goog-Api-Key", "{{secret:google-api-key}}");
    req.timeout_ms = Some(15_000);

    let resp = host::fetch(&req)?;
    let body = resp.text();
    check_http_status(resp.status, &body, "Google Cloud Translate")?;

    let v: Value = serde_json::from_str(&body)
        .map_err(|e| Error::Failed(format!("Google Cloud parse error: {e}")))?;
    let first = &v["data"]["translations"][0];
    let translation = first["translatedText"]
        .as_str()
        .ok_or_else(|| Error::Failed("Google Cloud response missing translatedText".into()))?
        .trim()
        .to_owned();
    let detected = first["detectedSourceLanguage"]
        .as_str()
        .map(normalize_detected_code)
        .unwrap_or_else(|| source_lang.to_owned());

    Ok(TranslationResult {
        translation,
        source_lang: if source_lang == "auto" {
            detected
        } else {
            source_lang.to_owned()
        },
        target_lang: target_lang.to_owned(),
        ..TranslationResult::default()
    })
}

fn strip_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_owned()
}

// ---------------------------------------------------------------------------
// 2. Microsoft Translator (Edge Free + Official Azure)
// ---------------------------------------------------------------------------

fn fetch_edge_token(force_refresh: bool, cache: &mut Option<EdgeToken>) -> Result<String, Error> {
    let now = unix_secs_now();
    if !force_refresh
        && let Some(cached) = cache.as_ref()
        && now + 30 < cached.expires_at_secs
    {
        return Ok(cached.token.clone());
    }

    let mut req = HttpRequest::get("https://edge.microsoft.com/translate/auth")
        .header("Accept", "text/plain");
    req.timeout_ms = Some(10_000);
    let resp = host::fetch(&req)?;
    let token = resp.text().trim().to_owned();
    if !(200..300).contains(&resp.status) || token.is_empty() {
        return Err(Error::Failed(format!(
            "Microsoft Edge auth failed (HTTP {}). Switch Microsoft mode to 'azure' with `microsoft-api-key` or select Google/OpenAI.",
            resp.status
        )));
    }
    let exp = parse_jwt_exp(&token).unwrap_or(now + 480);
    *cache = Some(EdgeToken {
        token: token.clone(),
        expires_at_secs: exp,
    });
    Ok(token)
}

fn parse_jwt_exp(jwt: &str) -> Option<u64> {
    let payload_b64 = jwt.split('.').nth(1)?;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("exp").and_then(Value::as_u64)
}

fn translate_microsoft(
    text: &str,
    source_lang: &str,
    target_lang: &str,
    cfg: &TranslationConfig,
    edge_token: &mut Option<EdgeToken>,
) -> Result<TranslationResult, Error> {
    let to = to_microsoft_code(target_lang).unwrap_or("en");
    let from = to_microsoft_code(source_lang);
    let mut query = format!("api-version=3.0&to={}", percent_encode_rfc3986(to));
    if let Some(f) = from {
        query.push_str(&format!("&from={}", percent_encode_rfc3986(f)));
    }
    let body_json = json!([{ "Text": text }]).to_string();

    let (resp_body, auth_mode_used, bearer_token) = if cfg.microsoft_mode == "azure" {
        if !host::secret_is_set("microsoft-api-key") {
            return Err(Error::Invalid(
                "Microsoft Azure mode requires secret `microsoft-api-key` in Settings -> Plugins"
                    .into(),
            ));
        }
        let base = if cfg.microsoft_custom_endpoint.is_empty() {
            "https://api.cognitive.microsofttranslator.com"
        } else {
            cfg.microsoft_custom_endpoint.trim_end_matches('/')
        };
        let url = format!("{base}/translate?{query}");
        let mut req = HttpRequest::post(&url, body_json)
            .header("Content-Type", "application/json")
            .header("Ocp-Apim-Subscription-Key", "{{secret:microsoft-api-key}}");
        if !cfg.microsoft_region.is_empty() && cfg.microsoft_region != "global" {
            req = req.header("Ocp-Apim-Subscription-Region", &cfg.microsoft_region);
        }
        req.timeout_ms = Some(15_000);
        let resp = host::fetch(&req)?;
        let txt = resp.text();
        check_http_status(resp.status, &txt, "Microsoft Azure Translator")?;
        (txt, true, None)
    } else {
        let mut token = fetch_edge_token(false, edge_token)?;
        let url = format!("https://api-edge.cognitive.microsofttranslator.com/translate?{query}");
        let mut req = HttpRequest::post(&url, body_json.clone())
            .header("Content-Type", "application/json")
            .header("Authorization", &format!("Bearer {token}"));
        req.timeout_ms = Some(15_000);
        let mut resp = host::fetch(&req)?;
        if resp.status == 401 || resp.status == 403 {
            token = fetch_edge_token(true, edge_token)?;
            let retry_req = HttpRequest::post(&url, body_json)
                .header("Content-Type", "application/json")
                .header("Authorization", &format!("Bearer {token}"));
            resp = host::fetch(&retry_req)?;
        }
        let txt = resp.text();
        check_http_status(resp.status, &txt, "Microsoft Translator")?;
        (txt, false, Some(token))
    };

    let v: Value = serde_json::from_str(&resp_body)
        .map_err(|e| Error::Failed(format!("Microsoft parse error: {e}")))?;
    let first = v
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| Error::Failed("Microsoft response array is empty".into()))?;
    let translation = first["translations"][0]["text"]
        .as_str()
        .ok_or_else(|| Error::Failed("Microsoft response missing translation text".into()))?
        .trim()
        .to_owned();
    let detected = first["detectedLanguage"]["language"]
        .as_str()
        .map(normalize_detected_code)
        .unwrap_or_else(|| source_lang.to_owned());
    let resolved_src = if source_lang == "auto" {
        detected
    } else {
        source_lang.to_owned()
    };

    let mut dict = Vec::new();
    let mut alternatives = Vec::new();
    if !text.contains('\n') && text.split_whitespace().count() <= 2 && text.chars().count() <= 40 {
        if let Some(ms_from) = to_microsoft_code(&resolved_src)
            && ms_from != to
        {
            let dict_base = if auth_mode_used {
                if cfg.microsoft_custom_endpoint.is_empty() {
                    "https://api.cognitive.microsofttranslator.com"
                } else {
                    cfg.microsoft_custom_endpoint.trim_end_matches('/')
                }
            } else {
                "https://api-edge.cognitive.microsofttranslator.com"
            };
            let dict_url = format!(
                "{dict_base}/dictionary/lookup?api-version=3.0&from={}&to={}",
                percent_encode_rfc3986(ms_from),
                percent_encode_rfc3986(to)
            );
            let mut dreq = HttpRequest::post(&dict_url, json!([{ "Text": text }]).to_string())
                .header("Content-Type", "application/json");
            if auth_mode_used {
                dreq = dreq.header("Ocp-Apim-Subscription-Key", "{{secret:microsoft-api-key}}");
            } else if let Some(tok) = bearer_token {
                dreq = dreq.header("Authorization", &format!("Bearer {tok}"));
            }
            dreq.timeout_ms = Some(6_000);
            if let Ok(dresp) = host::fetch(&dreq)
                && (200..300).contains(&dresp.status)
                && let Ok(dv) = serde_json::from_str::<Value>(&dresp.text())
                && let Some(items) = dv[0]["translations"].as_array()
            {
                let mut by_pos: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for item in items.iter().take(12) {
                    let pos = item["posTag"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if let Some(disp) = item["displayTarget"].as_str() {
                        let clean = disp.trim().to_owned();
                        if !clean.is_empty() {
                            if !alternatives.contains(&clean) && clean != translation {
                                alternatives.push(clean.clone());
                            }
                            by_pos.entry(pos).or_default().push(clean);
                        }
                    }
                }
                for (pos, terms) in by_pos {
                    dict.push(DictEntry {
                        pos,
                        terms,
                        explains: Vec::new(),
                    });
                }
            }
        }
    }

    Ok(TranslationResult {
        translation,
        source_lang: resolved_src,
        target_lang: target_lang.to_owned(),
        dict,
        alternatives,
        ..TranslationResult::default()
    })
}

// ---------------------------------------------------------------------------
// 3. OpenAI / Compatible LLM Translator
// ---------------------------------------------------------------------------

/// Renders TranslationPlugin Velocity-style prompt templates with:
/// - `#if($SOURCE_LANGUAGE)...#else...#end` conditionals (falsy when `source_lang == "auto"`)
/// - `${SOURCE_LANGUAGE}` / `$SOURCE_LANGUAGE`, `${TARGET_LANGUAGE}` / `$TARGET_LANGUAGE`,
///   `${TEXT}` / `$TEXT`, and `${DS}` literal `$` escape
/// - `[SYSTEM]` / `[USER]` message section markers
pub fn render_openai_prompt_template(
    template: &str,
    source_lang: &str,
    target_lang: &str,
    text: &str,
) -> (Option<String>, String) {
    let has_explicit_source = !source_lang.trim().is_empty() && source_lang != "auto";
    let src_name = if has_explicit_source {
        to_openai_name(source_lang)
    } else {
        String::new()
    };
    let tgt_name = to_openai_name(target_lang);

    let mut work = template.to_owned();
    if let Ok(if_else_re) = regex::Regex::new(
        r"(?s)#if\s*\(\s*\$\{?SOURCE_LANGUAGE\}?\s*\)(.*?)(?:#else(.*?))?#end",
    ) {
        work = if_else_re
            .replace_all(&work, |caps: &regex::Captures| {
                if has_explicit_source {
                    caps.get(1).map_or("", |m| m.as_str()).to_owned()
                } else {
                    caps.get(2).map_or("", |m| m.as_str()).to_owned()
                }
            })
            .into_owned();
    }

    const DOLLAR_SENTINEL: &str = "\x00OPENIDEA_DS\x00";
    work = work
        .replace("${DS}", DOLLAR_SENTINEL)
        .replace("$DS", DOLLAR_SENTINEL)
        .replace("${SOURCE_LANGUAGE}", &src_name)
        .replace("$SOURCE_LANGUAGE", &src_name)
        .replace("${TARGET_LANGUAGE}", &tgt_name)
        .replace("$TARGET_LANGUAGE", &tgt_name)
        .replace("${TEXT}", text)
        .replace("$TEXT", text)
        .replace(DOLLAR_SENTINEL, "$");

    let trimmed = work.trim();
    if let Some(after_sys) = trimmed.strip_prefix("[SYSTEM]") {
        if let Some((sys_raw, usr_raw)) = after_sys.split_once("[USER]") {
            let sys = sys_raw.trim().to_owned();
            let usr = usr_raw.trim().to_owned();
            return (
                if sys.is_empty() { None } else { Some(sys) },
                if usr.is_empty() { text.to_owned() } else { usr },
            );
        }
        return (None, after_sys.trim().to_owned());
    }
    if let Some((before_usr, after_usr)) = trimmed.split_once("[USER]") {
        let sys_candidate = before_usr.replace("[SYSTEM]", "").trim().to_owned();
        let usr = after_usr.trim().to_owned();
        return (
            if sys_candidate.is_empty() {
                None
            } else {
                Some(sys_candidate)
            },
            if usr.is_empty() { text.to_owned() } else { usr },
        );
    }

    (None, trimmed.to_owned())
}

fn translate_openai(
    text: &str,
    source_lang: &str,
    target_lang: &str,
    cfg: &TranslationConfig,
    is_document: bool,
) -> Result<TranslationResult, Error> {
    let base = cfg.openai_base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(Error::Invalid("OpenAI base URL is empty".into()));
    }
    let url = if base.ends_with("/chat/completions") {
        base.to_owned()
    } else {
        format!("{base}/chat/completions")
    };

    let has_secret = host::secret_is_set("openai-api-key");
    if !has_secret && !is_loopback_url(&url) {
        return Err(Error::Invalid(
            "OpenAI / compatible remote endpoint requires secret `openai-api-key` in Settings -> Plugins"
                .into(),
        ));
    }

    let custom_template = if is_document {
        cfg.openai_document_prompt.trim()
    } else {
        cfg.openai_translate_prompt.trim()
    };

    let is_short_phrase = !is_document
        && custom_template.is_empty()
        && cfg.openai_structured_dict
        && !text.contains('\n')
        && text.chars().count() <= 80
        && text.split_whitespace().count() <= 4;

    let (messages, using_custom_template) = if !custom_template.is_empty() {
        let (tpl_sys, tpl_usr) =
            render_openai_prompt_template(custom_template, source_lang, target_lang, text);
        let combined_sys = match (tpl_sys, cfg.openai_system_prompt.trim()) {
            (Some(s), "") => Some(s),
            (Some(s), extra) => Some(format!("{s}\n{extra}")),
            (None, "") => None,
            (None, extra) => Some(extra.to_owned()),
        };
        let mut msgs = Vec::new();
        if let Some(sys) = combined_sys {
            msgs.push(json!({ "role": "system", "content": sys }));
        }
        msgs.push(json!({ "role": "user", "content": tpl_usr }));
        (msgs, true)
    } else {
        let src_desc = to_openai_name(source_lang);
        let tgt_desc = to_openai_name(target_lang);
        let mut sys_prompt = if is_short_phrase {
            format!(
                "You are an expert technical translator & bilingual dictionary. Translate from {src_desc} to {tgt_desc}. \
                 Respond ONLY with valid JSON matching: \
                 {{\"translation\":\"primary concise translation\",\"detectedLang\":\"ISO code\",\"phonetic\":\"IPA or empty\",\"dict\":[{{\"pos\":\"noun/verb/adj\",\"terms\":[\"alt1\",\"alt2\"],\"explains\":[\"concise sense\"]}}],\"alternatives\":[\"alt\"],\"examples\":[\"short bilingual example\"]}}."
            )
        } else if is_document {
            format!(
                "You are an expert software documentation translator. Translate from {src_desc} to {tgt_desc}. \
                 Preserve Markdown headings/lists/links, inline code, URLs, comment tokens, and any `[[TT_n]]` placeholders verbatim. Output ONLY the translated content without extra commentary."
            )
        } else {
            format!(
                "You are an expert software engineering translator. Translate the user's text from {src_desc} to {tgt_desc}. \
                 Preserve Markdown formatting, inline code, URLs, placeholders, and identifier tokens accurately. Output ONLY the translated text without commentary."
            )
        };
        if !cfg.openai_system_prompt.trim().is_empty() {
            sys_prompt.push('\n');
            sys_prompt.push_str(cfg.openai_system_prompt.trim());
        }
        (
            vec![
                json!({ "role": "system", "content": sys_prompt }),
                json!({ "role": "user", "content": text }),
            ],
            false,
        )
    };

    let payload = json!({
        "model": cfg.openai_model,
        "temperature": cfg.openai_temperature,
        "messages": messages
    });

    let mut req = HttpRequest::post(&url, payload.to_string())
        .header("Content-Type", "application/json");
    if has_secret {
        req = req.header("Authorization", "Bearer {{secret:openai-api-key}}");
    }
    req.timeout_ms = Some(30_000);

    let resp = host::fetch(&req)?;
    let body = resp.text();
    check_http_status(resp.status, &body, "OpenAI")?;

    let v: Value = serde_json::from_str(&body)
        .map_err(|e| Error::Failed(format!("OpenAI response parse error: {e}")))?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| Error::Failed("OpenAI response missing choices[0].message.content".into()))?
        .trim();

    if (is_short_phrase || !using_custom_template)
        && let Some(parsed) = try_parse_openai_dict(content, source_lang, target_lang)
        && is_short_phrase
    {
        return Ok(parsed);
    }

    Ok(TranslationResult {
        translation: strip_markdown_code_fence(content),
        source_lang: source_lang.to_owned(),
        target_lang: target_lang.to_owned(),
        ..TranslationResult::default()
    })
}

fn strip_markdown_code_fence(s: &str) -> String {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let after_lang = rest
            .split_once('\n')
            .map_or(rest, |(_, body)| body);
        if let Some(inner) = after_lang.strip_suffix("```") {
            return inner.trim().to_owned();
        }
    }
    trimmed.to_owned()
}

fn try_parse_openai_dict(
    content: &str,
    source_lang: &str,
    target_lang: &str,
) -> Option<TranslationResult> {
    let cleaned = strip_markdown_code_fence(content);
    let start = cleaned.find('{')?;
    let end = cleaned.rfind('}')?;
    let slice = &cleaned[start..=end];
    let v: Value = serde_json::from_str(slice).ok()?;
    let translation = v.get("translation").and_then(Value::as_str)?.trim().to_owned();
    if translation.is_empty() {
        return None;
    }

    let detected = v
        .get("detectedLang")
        .and_then(Value::as_str)
        .map(normalize_detected_code)
        .unwrap_or_else(|| source_lang.to_owned());
    let phonetic = v
        .get("phonetic")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let mut dict = Vec::new();
    if let Some(arr) = v.get("dict").and_then(Value::as_array) {
        for item in arr.iter().take(6) {
            let pos = item.get("pos").and_then(Value::as_str).unwrap_or("").trim().to_owned();
            let terms = item
                .get("terms")
                .and_then(Value::as_array)
                .map(|t| {
                    t.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let explains = item
                .get("explains")
                .and_then(Value::as_array)
                .map(|t| {
                    t.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            dict.push(DictEntry { pos, terms, explains });
        }
    }

    let alternatives = v
        .get("alternatives")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty() && s != &translation)
                .collect()
        })
        .unwrap_or_default();

    let examples = v
        .get("examples")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| ExampleItem {
                    source: s.trim().to_owned(),
                    target: None,
                })
                .filter(|e| !e.source.is_empty())
                .collect()
        })
        .unwrap_or_default();

    Some(TranslationResult {
        translation,
        source_lang: if source_lang == "auto" {
            detected
        } else {
            source_lang.to_owned()
        },
        target_lang: target_lang.to_owned(),
        phonetics: Phonetics {
            general: phonetic,
            ..Phonetics::default()
        },
        dict,
        alternatives,
        examples,
        ..TranslationResult::default()
    })
}

// ---------------------------------------------------------------------------
// 4. DeepL API (Free / Pro / Custom)
// ---------------------------------------------------------------------------

fn translate_deepl(
    text: &str,
    source_lang: &str,
    target_lang: &str,
    cfg: &TranslationConfig,
) -> Result<TranslationResult, Error> {
    if !host::secret_is_set("deepl-api-key") {
        return Err(Error::Invalid(
            "DeepL requires secret `deepl-api-key` in Settings -> Plugins".into(),
        ));
    }

    let url = match cfg.deepl_tier.as_str() {
        "pro" => "https://api.deepl.com/v2/translate".to_owned(),
        "custom" => {
            if cfg.deepl_custom_url.is_empty() {
                return Err(Error::Invalid(
                    "DeepL custom tier selected but `deeplCustomUrl` is empty".into(),
                ));
            }
            cfg.deepl_custom_url.clone()
        }
        _ => "https://api-free.deepl.com/v2/translate".to_owned(),
    };

    let tgt_code = to_deepl_target(target_lang);
    let mut payload = json!({
        "text": [text],
        "target_lang": tgt_code
    });
    if let Some(src_code) = to_deepl_source(source_lang) {
        payload["source_lang"] = Value::String(src_code.to_owned());
    }
    if cfg.deepl_formality != "default" && deepl_supports_formality(tgt_code) {
        payload["formality"] = Value::String(cfg.deepl_formality.clone());
    }

    let mut req = HttpRequest::post(&url, payload.to_string())
        .header("Content-Type", "application/json")
        .header("Authorization", "DeepL-Auth-Key {{secret:deepl-api-key}}");
    req.timeout_ms = Some(15_000);

    let resp = host::fetch(&req)?;
    let body = resp.text();
    check_http_status(resp.status, &body, "DeepL")?;

    let v: Value = serde_json::from_str(&body)
        .map_err(|e| Error::Failed(format!("DeepL parse error: {e}")))?;
    let first = &v["translations"][0];
    let translation = first["text"]
        .as_str()
        .ok_or_else(|| Error::Failed("DeepL response missing translations[0].text".into()))?
        .trim()
        .to_owned();
    let detected = first["detected_source_language"]
        .as_str()
        .map(normalize_detected_code)
        .unwrap_or_else(|| source_lang.to_owned());

    Ok(TranslationResult {
        translation,
        source_lang: if source_lang == "auto" {
            detected
        } else {
            source_lang.to_owned()
        },
        target_lang: target_lang.to_owned(),
        ..TranslationResult::default()
    })
}

// ---------------------------------------------------------------------------
// 5. Youdao Zhiyun API v3 (SHA-256)
// ---------------------------------------------------------------------------

fn translate_youdao(
    text: &str,
    source_lang: &str,
    target_lang: &str,
    cfg: &TranslationConfig,
) -> Result<TranslationResult, Error> {
    if cfg.youdao_app_key.is_empty() || cfg.youdao_app_secret.is_empty() {
        return Err(Error::Invalid(
            "Youdao Translate requires `youdaoAppKey` and `youdaoAppSecret` in Settings -> Translation -> Engines & AI"
                .into(),
        ));
    }

    let salt = next_nonce();
    let curtime = unix_secs_now().to_string();
    let sign = youdao_v3_sign(
        &cfg.youdao_app_key,
        text,
        &salt,
        &curtime,
        &cfg.youdao_app_secret,
    );
    let from = to_youdao_code(source_lang);
    let to = to_youdao_code(target_lang);

    let body = form_urlencode(&[
        ("q", text),
        ("from", from),
        ("to", to),
        ("appKey", &cfg.youdao_app_key),
        ("salt", &salt),
        ("sign", &sign),
        ("signType", "v3"),
        ("curtime", &curtime),
    ]);

    let mut req = HttpRequest::post("https://openapi.youdao.com/api", body)
        .header("Content-Type", "application/x-www-form-urlencoded");
    req.timeout_ms = Some(15_000);

    let resp = host::fetch(&req)?;
    let resp_text = resp.text();
    check_http_status(resp.status, &resp_text, "Youdao Translate")?;

    let v: Value = serde_json::from_str(&resp_text)
        .map_err(|e| Error::Failed(format!("Youdao response parse error: {e}")))?;
    let err_code = v.get("errorCode").and_then(Value::as_str).unwrap_or("-1");
    if err_code != "0" {
        return Err(Error::Failed(format!("Youdao API errorCode={err_code}")));
    }

    let translation = v
        .get("translation")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
        .trim()
        .to_owned();
    if translation.is_empty() {
        return Err(Error::Failed("Youdao returned empty translation".into()));
    }

    let mut phonetics = Phonetics::default();
    let mut dict = Vec::new();
    let mut alternatives = Vec::new();

    if let Some(basic) = v.get("basic") {
        phonetics.us = basic
            .get("us-phonetic")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        phonetics.uk = basic
            .get("uk-phonetic")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        phonetics.general = basic
            .get("phonetic")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());

        if let Some(explains) = basic.get("explains").and_then(Value::as_array) {
            let lines: Vec<String> = explains
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            for line in &lines {
                if let Some((pos, rest)) = split_pos_prefix(line) {
                    let terms: Vec<String> = rest
                        .split(['；', ';', '，', ','])
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                    for t in terms.iter().take(3) {
                        if !alternatives.contains(t) && t != &translation {
                            alternatives.push(t.clone());
                        }
                    }
                    dict.push(DictEntry {
                        pos,
                        terms,
                        explains: vec![line.clone()],
                    });
                } else {
                    dict.push(DictEntry {
                        pos: String::new(),
                        terms: vec![line.clone()],
                        explains: vec![line.clone()],
                    });
                }
            }
        }
    }

    if let Some(web_arr) = v.get("web").and_then(Value::as_array) {
        let web_terms: Vec<String> = web_arr
            .iter()
            .filter_map(|item| {
                let k = item.get("key").and_then(Value::as_str)?;
                let vals: Vec<&str> = item
                    .get("value")
                    .and_then(Value::as_array)?
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                Some(format!("{k}: {}", vals.join("; ")))
            })
            .collect();
        if !web_terms.is_empty() {
            dict.push(DictEntry {
                pos: "web".into(),
                terms: Vec::new(),
                explains: web_terms,
            });
        }
    }

    let detected = v
        .get("l")
        .and_then(Value::as_str)
        .and_then(|pair| pair.split('2').next())
        .map(normalize_detected_code)
        .unwrap_or_else(|| source_lang.to_owned());

    Ok(TranslationResult {
        translation,
        source_lang: if source_lang == "auto" {
            detected
        } else {
            source_lang.to_owned()
        },
        target_lang: target_lang.to_owned(),
        phonetics,
        dict,
        alternatives,
        ..TranslationResult::default()
    })
}

fn split_pos_prefix(line: &str) -> Option<(String, &str)> {
    let (prefix, rest) = line.split_once('.')?;
    let trimmed = prefix.trim();
    if !trimmed.is_empty()
        && trimmed.len() <= 8
        && trimmed.chars().all(|c| c.is_ascii_alphabetic() || c == '/')
    {
        Some((format!("{trimmed}."), rest.trim()))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 6. Baidu Translate API (MD5)
// ---------------------------------------------------------------------------

fn translate_baidu(
    text: &str,
    source_lang: &str,
    target_lang: &str,
    cfg: &TranslationConfig,
) -> Result<TranslationResult, Error> {
    if cfg.baidu_app_id.is_empty() || cfg.baidu_secret_key.is_empty() {
        return Err(Error::Invalid(
            "Baidu Translate requires `baiduAppId` and `baiduSecretKey` in Settings -> Translation -> Engines & AI"
                .into(),
        ));
    }

    let salt = next_nonce();
    let sign = baidu_md5_sign(&cfg.baidu_app_id, text, &salt, &cfg.baidu_secret_key);
    let from = to_baidu_code(source_lang);
    let to = to_baidu_code(target_lang);

    let body = form_urlencode(&[
        ("q", text),
        ("from", from),
        ("to", to),
        ("appid", &cfg.baidu_app_id),
        ("salt", &salt),
        ("sign", &sign),
    ]);

    let mut req = HttpRequest::post("https://fanyi-api.baidu.com/api/trans/vip/translate", body)
        .header("Content-Type", "application/x-www-form-urlencoded");
    req.timeout_ms = Some(15_000);

    let resp = host::fetch(&req)?;
    let resp_text = resp.text();
    check_http_status(resp.status, &resp_text, "Baidu Translate")?;

    let v: Value = serde_json::from_str(&resp_text)
        .map_err(|e| Error::Failed(format!("Baidu parse error: {e}")))?;
    if let Some(code) = v.get("error_code") {
        let msg = v
            .get("error_msg")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(Error::Failed(format!("Baidu API error {code}: {msg}")));
    }

    let translation = v
        .get("trans_result")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("dst").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
        .trim()
        .to_owned();
    if translation.is_empty() {
        return Err(Error::Failed("Baidu returned empty translation".into()));
    }

    let detected = v
        .get("from")
        .and_then(Value::as_str)
        .map(normalize_detected_code)
        .unwrap_or_else(|| source_lang.to_owned());

    Ok(TranslationResult {
        translation,
        source_lang: if source_lang == "auto" {
            detected
        } else {
            source_lang.to_owned()
        },
        target_lang: target_lang.to_owned(),
        ..TranslationResult::default()
    })
}

// ---------------------------------------------------------------------------
// 7. Alibaba Cloud Machine Translation (RPC HMAC-SHA1)
// ---------------------------------------------------------------------------

fn translate_alibaba(
    text: &str,
    source_lang: &str,
    target_lang: &str,
    cfg: &TranslationConfig,
) -> Result<TranslationResult, Error> {
    if cfg.alibaba_access_key_id.is_empty() || cfg.alibaba_access_key_secret.is_empty() {
        return Err(Error::Invalid(
            "Alibaba Cloud MT requires `alibabaAccessKeyId` and `alibabaAccessKeySecret` in Settings -> Translation -> Engines & AI"
                .into(),
        ));
    }

    let mut params = BTreeMap::new();
    params.insert("AccessKeyId".into(), cfg.alibaba_access_key_id.clone());
    params.insert("Action".into(), "TranslateGeneral".into());
    params.insert("Format".into(), "JSON".into());
    params.insert("FormatType".into(), "text".into());
    params.insert("RegionId".into(), cfg.alibaba_region_id.clone());
    params.insert("Scene".into(), cfg.alibaba_scene.clone());
    params.insert("SignatureMethod".into(), "HMAC-SHA1".into());
    params.insert("SignatureNonce".into(), next_nonce());
    params.insert("SignatureVersion".into(), "1.0".into());
    params.insert("SourceLanguage".into(), to_alibaba_code(source_lang).to_owned());
    params.insert("SourceText".into(), text.to_owned());
    params.insert("TargetLanguage".into(), to_alibaba_code(target_lang).to_owned());
    params.insert("Timestamp".into(), format_utc_iso8601(unix_secs_now()));
    params.insert("Version".into(), "2018-10-12".into());

    let signature = alibaba_rpc_sign("POST", &params, &cfg.alibaba_access_key_secret)?;
    params.insert("Signature".into(), signature);

    let pairs: Vec<(&str, &str)> = params
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let body = form_urlencode(&pairs);

    let endpoint = if cfg.alibaba_region_id.is_empty() {
        "https://mt.cn-hangzhou.aliyuncs.com/".to_owned()
    } else {
        format!("https://mt.{}.aliyuncs.com/", cfg.alibaba_region_id)
    };
    let mut req = HttpRequest::post(&endpoint, body)
        .header("Content-Type", "application/x-www-form-urlencoded");
    req.timeout_ms = Some(15_000);

    let resp = host::fetch(&req)?;
    let resp_text = resp.text();
    check_http_status(resp.status, &resp_text, "Alibaba Cloud MT")?;

    let v: Value = serde_json::from_str(&resp_text)
        .map_err(|e| Error::Failed(format!("Alibaba parse error: {e}")))?;
    let code = v.get("Code").and_then(Value::as_str).unwrap_or("");
    if !code.is_empty() && code != "200" {
        let msg = v.get("Message").and_then(Value::as_str).unwrap_or("");
        return Err(Error::Failed(format!("Alibaba API Code={code}: {msg}")));
    }

    let data = v.get("Data").unwrap_or(&Value::Null);
    let translation = data
        .get("Translated")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Failed("Alibaba response missing Data.Translated".into()))?
        .trim()
        .to_owned();
    let detected = data
        .get("DetectedLanguage")
        .and_then(Value::as_str)
        .map(normalize_detected_code)
        .unwrap_or_else(|| source_lang.to_owned());

    Ok(TranslationResult {
        translation,
        source_lang: if source_lang == "auto" {
            detected
        } else {
            source_lang.to_owned()
        },
        target_lang: target_lang.to_owned(),
        ..TranslationResult::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::HttpResponse;
    use openidea_plugin::mock::MockHost;

    #[test]
    fn verifies_youdao_truncate_and_sha256_vector() {
        assert_eq!(youdao_truncate("hello"), "hello");
        assert_eq!(youdao_truncate("12345678901234567890"), "12345678901234567890");
        assert_eq!(
            youdao_truncate("1234567890abcdefghij12345"),
            "123456789025fghij12345"
        );
        // Check exact 21-char UTF-8 truncation uses char count, not byte length
        let chinese_22 = "一二三四五六七八九十甲乙丙丁戊己庚辛壬癸子丑";
        assert_eq!(youdao_truncate(chinese_22), "一二三四五六七八九十22丙丁戊己庚辛壬癸子丑");

        let sig = youdao_v3_sign("app1", "hello", "salt1", "1700000000", "sec1");
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn verifies_alibaba_rfc3986_hmac_sha1_signature() {
        let mut params = BTreeMap::new();
        params.insert("Action".into(), "TranslateGeneral".into());
        params.insert("SourceText".into(), "Hello World!".into());
        let sig = alibaba_rpc_sign("POST", &params, "testsecret").unwrap();
        // Canonical query: Action=TranslateGeneral&SourceText=Hello%20World%21
        let expected_canonical = "Action=TranslateGeneral&SourceText=Hello%20World%21";
        let string_to_sign = format!(
            "POST&%2F&{}",
            percent_encode_rfc3986(expected_canonical)
        );
        let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(b"testsecret&").unwrap();
        mac.update(string_to_sign.as_bytes());
        assert_eq!(sig, STANDARD.encode(mac.finalize().into_bytes()));
        assert_eq!(format_utc_iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc_iso8601(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn google_free_parses_rich_dictionary_alternatives_and_spell() {
        let host = MockHost::install().with_capabilities(&["storage", "net:translate.googleapis.com"]);
        host.on_fetch(|req| {
            assert!(req.url.contains("translate.googleapis.com/translate_a/single"));
            assert!(req.url.contains("dt=at"));
            assert!(req.url.contains("dt=qca"));
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: r#"{
                    "sentences":[{"trans":"缓存","orig":"cach","src_translit":"kaʃ"}],
                    "src":"en",
                    "spell":{"spell_html_res":"<b><i>cache</i></b>","spell_res":"cache"},
                    "alternative_translations":[{"alternative":[{"word_postproc":"缓存"},{"word_postproc":"高速缓冲存储器"}]}],
                    "dict":[{"pos":"noun","terms":["缓存","高速缓存"],"entry":[{"word":"缓存","reverse_translation":["cache"]}]}],
                    "synsets":[{"entry":[{"synonym":["store","buffer"]}]}],
                    "examples":{"example":[{"text":"Clear the <b>cache</b> first."}]}
                }"#.as_bytes().to_vec(),
            })
        });
        let cfg = TranslationConfig::load();
        let mut edge = None;
        let res = translate_with_engine(EngineId::Google, "cach", "cach", "auto", "zh-CN", &cfg, &mut edge).unwrap();
        assert_eq!(res.translation, "缓存");
        assert_eq!(res.source_lang, "en");
        assert_eq!(res.spell_correction.as_deref(), Some("cache"));
        assert!(res.alternatives.contains(&"高速缓冲存储器".to_owned()));
        assert_eq!(res.phonetics.general.as_deref(), Some("kaʃ"));
        assert_eq!(res.dict[0].pos, "noun");
        assert_eq!(res.synonyms, vec!["store", "buffer"]);
        assert_eq!(res.examples[0].source, "Clear the cache first.");
    }

    #[test]
    fn openai_velocity_prompt_template_renders_conditionals_and_sections() {
        let tpl = "[SYSTEM]\nRule ${DS}1\n[USER]\n#if($SOURCE_LANGUAGE)From ${SOURCE_LANGUAGE} to ${TARGET_LANGUAGE}:#elseTo ${TARGET_LANGUAGE}:#end ${TEXT}";
        let (sys_auto, usr_auto) = render_openai_prompt_template(tpl, "auto", "zh-CN", "hello");
        assert_eq!(sys_auto.as_deref(), Some("Rule $1"));
        assert_eq!(usr_auto, "To Simplified Chinese (zh-CN): hello");

        let (_sys_explicit, usr_explicit) =
            render_openai_prompt_template(tpl, "en", "ja", "hello");
        assert_eq!(usr_explicit, "From English (en) to Japanese (ja): hello");
    }

    #[test]
    fn microsoft_edge_and_azure_work_with_token_caching() {
        let host = MockHost::install().with_capabilities(&[
            "storage",
            "secrets",
            "net:edge.microsoft.com",
            "net:api-edge.cognitive.microsofttranslator.com",
            "net:api.cognitive.microsofttranslator.com",
        ]);
        host.on_fetch(|req| {
            if req.url.contains("edge.microsoft.com/translate/auth") {
                Ok(HttpResponse { status: 200, headers: vec![], body: b"edge.jwt.sig".to_vec() })
            } else if req.url.contains("/dictionary/lookup") {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: r#"[{"translations":[{"displayTarget":"令牌","posTag":"NOUN"}]}]"#.as_bytes().to_vec(),
                })
            } else {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: r#"[{"detectedLanguage":{"language":"en"},"translations":[{"text":"令牌","to":"zh-Hans"}]}]"#.as_bytes().to_vec(),
                })
            }
        });
        let cfg = TranslationConfig::load();
        let mut edge = None;
        let r1 = translate_with_engine(EngineId::Microsoft, "token", "token", "auto", "zh-CN", &cfg, &mut edge).unwrap();
        assert_eq!(r1.translation, "令牌");
        assert_eq!(r1.dict[0].pos, "noun");
        // Second call reuses cached Edge token
        let before = host.requests().len();
        let _r2 = translate_with_engine(EngineId::Microsoft, "token", "token", "auto", "zh-CN", &cfg, &mut edge).unwrap();
        let new_reqs = &host.requests()[before..];
        assert!(!new_reqs.iter().any(|r| r.url.contains("/translate/auth")));

        // Azure mode injects secret header
        host.set_setting("microsoftMode", "\"azure\"");
        host.set_setting("microsoftRegion", "\"eastasia\"");
        host.set_secret("microsoft-api-key", "az-secret");
        let cfg_az = TranslationConfig::load();
        let r3 = translate_with_engine(EngineId::Microsoft, "token", "token", "en", "zh-CN", &cfg_az, &mut edge).unwrap();
        assert_eq!(r3.translation, "令牌");
        let az_req = host.requests().into_iter().rfind(|r| r.url.contains("api.cognitive.microsofttranslator.com/translate")).unwrap();
        assert!(az_req.headers.contains(&("Ocp-Apim-Subscription-Key".into(), "az-secret".into())));
        assert!(az_req.headers.contains(&("Ocp-Apim-Subscription-Region".into(), "eastasia".into())));
    }

    #[test]
    fn openai_supports_structured_dict_ollama_and_fallback() {
        let host = MockHost::install().with_capabilities(&["storage", "secrets", "net:api.openai.com", "net:localhost"]);
        host.set_secret("openai-api-key", "sk-openai-1");
        host.on_fetch(|req| {
            let body: Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
            let user_text = body["messages"][1]["content"].as_str().unwrap();
            if user_text == "idempotent" {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: serde_json::json!({
                        "choices": [{
                            "message": {
                                "content": "```json\n{\"translation\":\"幂等的\",\"detectedLang\":\"en\",\"phonetic\":\"/ˌaɪdɛmˈpoʊtənt/\",\"dict\":[{\"pos\":\"adj.\",\"terms\":[\"幂等的\"]}],\"alternatives\":[\"等幂的\"],\"examples\":[\"Idempotent operations are safe to retry.\"]}\n```"
                            }
                        }]
                    }).to_string().into_bytes(),
                })
            } else {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: serde_json::json!({
                        "choices": [{ "message": { "content": "普通文本译文" } }]
                    }).to_string().into_bytes(),
                })
            }
        });
        let cfg = TranslationConfig::load();
        let mut edge = None;
        let dict_res = translate_with_engine(EngineId::Openai, "idempotent", "idempotent", "auto", "zh-CN", &cfg, &mut edge).unwrap();
        assert_eq!(dict_res.translation, "幂等的");
        assert_eq!(dict_res.phonetics.general.as_deref(), Some("/ˌaɪdɛmˈpoʊtənt/"));
        assert_eq!(dict_res.alternatives, vec!["等幂的"]);

        // Fallback plain text even on short phrase if local model returns plain text
        host.set_setting("openaiBaseUrl", "\"http://localhost:11434/v1\"");
        let cfg_local = TranslationConfig::load();
        let plain_res = translate_with_engine(EngineId::Openai, "hello", "hello", "auto", "zh-CN", &cfg_local, &mut edge).unwrap();
        assert_eq!(plain_res.translation, "普通文本译文");
    }

    #[test]
    fn deepl_youdao_baidu_alibaba_all_work() {
        let host = MockHost::install().with_capabilities(&[
            "storage",
            "secrets",
            "net:api-free.deepl.com",
            "net:openapi.youdao.com",
            "net:fanyi-api.baidu.com",
            "net:mt.cn-hangzhou.aliyuncs.com",
        ]);
        host.set_secret("deepl-api-key", "dl-key");
        host.set_setting("deeplFormality", "\"more\"");
        host.set_setting("youdaoAppKey", "\"yd-app\"");
        host.set_setting("youdaoAppSecret", "\"yd-sec\"");
        host.set_setting("baiduAppId", "\"bd-app\"");
        host.set_setting("baiduSecretKey", "\"bd-sec\"");
        host.set_setting("alibabaAccessKeyId", "\"ali-id\"");
        host.set_setting("alibabaAccessKeySecret", "\"ali-sec\"");

        host.on_fetch(|req| {
            if req.url.contains("api-free.deepl.com") {
                let body: Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
                // ZH-HANS does not support formality so DeepL skips it!
                assert!(body.get("formality").is_none());
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: r#"{"translations":[{"detected_source_language":"EN","text":"你好"}]}"#.as_bytes().to_vec(),
                })
            } else if req.url.contains("openapi.youdao.com") {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: r#"{"errorCode":"0","l":"en2zh-CHS","translation":["你好"],"basic":{"us-phonetic":"həˈloʊ","explains":["int. 你好；哈喽"]}}"#.as_bytes().to_vec(),
                })
            } else if req.url.contains("fanyi-api.baidu.com") {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: r#"{"from":"en","to":"zh","trans_result":[{"src":"hello","dst":"你好"}]}"#.as_bytes().to_vec(),
                })
            } else {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: r#"{"Code":"200","Data":{"Translated":"你好","DetectedLanguage":"en"}}"#.as_bytes().to_vec(),
                })
            }
        });

        let cfg = TranslationConfig::load();
        let mut edge = None;
        for engine in [EngineId::Deepl, EngineId::Youdao, EngineId::Baidu, EngineId::Alibaba] {
            let r = translate_with_engine(engine, "hello", "hello", "auto", "zh-CN", &cfg, &mut edge).unwrap();
            assert_eq!(r.translation, "你好");
            assert_eq!(r.source_lang, "en");
        }
    }
}
