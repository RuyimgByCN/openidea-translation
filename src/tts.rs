use crate::config::{TranslationConfig, TtsEngineId, is_loopback_url};
use crate::engines::{percent_encode_rfc3986, unix_millis_now, unix_secs_now};
use crate::languages::detect_script_language;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use openidea_plugin::host::{self, WebSocket, WebSocketRequest};
use openidea_plugin::{Error, HttpRequest};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

const EDGE_TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const EDGE_SEC_MS_GEC_VERSION: &str = "1-130.0.2849.68";
const WIN_EPOCH_OFFSET_SECS: u64 = 11_644_473_600;
const DRAIN_LIMIT: u32 = 32;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum TtsAudio {
    #[serde(rename_all = "camelCase")]
    DataUri {
        engine: String,
        mime: String,
        data_uri: String,
        rate: f64,
    },
    #[serde(rename_all = "camelCase")]
    Browser {
        engine: String,
        text: String,
        lang: String,
        rate: f64,
    },
}

pub struct PendingEdgeTts {
    pub ws: WebSocket,
    pub audio_chunks: Vec<u8>,
    pub text: String,
    pub lang: String,
    pub accent: String,
    pub rate: f64,
    pub created_ms: u64,
}

pub enum TtsSynthesis {
    Ready(TtsAudio),
    PendingEdge(PendingEdgeTts),
}

/// Computes Microsoft Edge TTS `Sec-MS-GEC` SHA-256 token quantized to 5-minute windows.
pub fn compute_sec_ms_gec(unix_secs: u64) -> String {
    let mut ticks_secs = unix_secs.saturating_add(WIN_EPOCH_OFFSET_SECS);
    ticks_secs -= ticks_secs % 300;
    let ticks_100ns = u128::from(ticks_secs) * 10_000_000u128;
    let payload = format!("{ticks_100ns}{EDGE_TRUSTED_CLIENT_TOKEN}");
    let digest = Sha256::digest(payload.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

pub fn select_edge_voice(voice_pref: &str, lang: &str, accent: &str) -> String {
    let pref = voice_pref.trim();
    if !pref.is_empty() && !pref.eq_ignore_ascii_case("auto") {
        return pref.to_owned();
    }
    match lang {
        "en" if accent == "uk" => "en-GB-SoniaNeural".to_owned(),
        "en" => "en-US-AriaNeural".to_owned(),
        "zh-TW" => "zh-TW-HsiaoChenNeural".to_owned(),
        "zh-CN" | "zh" => "zh-CN-XiaoxiaoNeural".to_owned(),
        "ja" => "ja-JP-NanamiNeural".to_owned(),
        "ko" => "ko-KR-SunHiNeural".to_owned(),
        "fr" => "fr-FR-DeniseNeural".to_owned(),
        "de" => "de-DE-KatjaNeural".to_owned(),
        "es" => "es-ES-ElviraNeural".to_owned(),
        "ru" => "ru-RU-SvetlanaNeural".to_owned(),
        "pt" => "pt-BR-FranciscaNeural".to_owned(),
        "it" => "it-IT-ElsaNeural".to_owned(),
        "ar" => "ar-SA-ZariyahNeural".to_owned(),
        _ => "en-US-AriaNeural".to_owned(),
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn prosody_rate_pct(rate: f64) -> String {
    let pct = ((rate.clamp(0.5, 2.0) - 1.0) * 100.0).round() as i32;
    if pct >= 0 {
        format!("+{pct}%")
    } else {
        format!("{pct}%")
    }
}

/// Builds `Path:speech.config` and `Path:ssml` protocol frames for Edge ReadAloud WebSocket.
pub fn build_edge_ssml(
    text: &str,
    voice: &str,
    rate: f64,
    request_id: &str,
    timestamp_rfc: &str,
) -> (String, String) {
    let config_msg = format!(
        "X-Timestamp:{timestamp_rfc}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n\
         {{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"false\"}},\"outputFormat\":\"audio-24khz-48kbitrate-mono-mp3\"}}}}}}}}\r\n"
    );
    let ssml_body = build_azure_ssml_doc(text, voice, rate);
    let ssml_msg = format!(
        "X-RequestId:{request_id}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{timestamp_rfc}\r\nPath:ssml\r\n\r\n{ssml_body}"
    );
    (config_msg, ssml_msg)
}

fn build_azure_ssml_doc(text: &str, voice: &str, rate: f64) -> String {
    let lang = voice
        .split('-')
        .take(2)
        .collect::<Vec<_>>()
        .join("-");
    let xml_lang = if lang.is_empty() { "en-US".to_owned() } else { lang };
    let rate_str = prosody_rate_pct(rate);
    format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{xml_lang}'>\
         <voice name='{voice}'><prosody rate='{rate_str}'>{}</prosody></voice></speak>",
        xml_escape(text)
    )
}

/// Parses an Edge TTS WebSocket binary frame (`[2-byte header len BE][headers][audio payload]`).
pub fn extract_edge_audio_chunk(frame: &[u8]) -> Option<&[u8]> {
    if frame.len() < 2 {
        return None;
    }
    let header_len = u16::from_be_bytes([frame[0], frame[1]]) as usize;
    let start = 2 + header_len;
    if start <= frame.len() {
        let header = &frame[2..start];
        if header.windows(b"Path:audio".len()).any(|w| w == b"Path:audio") && start < frame.len() {
            return Some(&frame[start..]);
        }
        return None;
    }
    None
}

/// Synchronous TTS helper retained for compatibility; falls back when Edge WS would block.
pub fn synthesize(
    text: &str,
    lang_hint: Option<&str>,
    accent_override: Option<&str>,
    cfg: &TranslationConfig,
) -> Result<TtsAudio, Error> {
    match synthesize_with_ws(text, lang_hint, accent_override, cfg)? {
        TtsSynthesis::Ready(audio) => Ok(audio),
        TtsSynthesis::PendingEdge(pending) => Ok(TtsAudio::Browser {
            engine: "browser".into(),
            text: pending.text,
            lang: to_bcp47(&pending.lang).to_owned(),
            rate: pending.rate,
        }),
    }
}

/// Full TTS dispatcher supporting Edge WebSocket, Azure Speech Neural TTS, Youdao,
/// chunked Google TTS, independent OpenAI TTS, and browser SpeechSynthesis.
pub fn synthesize_with_ws(
    text: &str,
    lang_hint: Option<&str>,
    accent_override: Option<&str>,
    cfg: &TranslationConfig,
) -> Result<TtsSynthesis, Error> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(Error::Invalid("Nothing to pronounce".into()));
    }
    let clipped: String = trimmed.chars().take(1200).collect();
    let lang = match lang_hint {
        Some(l) if !l.trim().is_empty() && l != "auto" => l.to_owned(),
        _ => detect_script_language(&clipped).to_owned(),
    };
    let accent = accent_override
        .filter(|a| *a == "us" || *a == "uk")
        .unwrap_or(&cfg.tts_accent);

    if cfg.tts_engine == TtsEngineId::Browser {
        return Ok(TtsSynthesis::Ready(TtsAudio::Browser {
            engine: "browser".into(),
            text: clipped,
            lang: to_bcp47(&lang).to_owned(),
            rate: cfg.tts_speed,
        }));
    }

    if cfg.tts_engine == TtsEngineId::Edge {
        return start_edge_tts_ws(&clipped, &lang, accent, cfg);
    }

    let chosen = match cfg.tts_engine {
        TtsEngineId::Azure => TtsEngineId::Azure,
        TtsEngineId::Youdao => TtsEngineId::Youdao,
        TtsEngineId::Google => TtsEngineId::Google,
        TtsEngineId::Openai => TtsEngineId::Openai,
        TtsEngineId::Auto => {
            if lang == "en" && clipped.len() <= 240 {
                TtsEngineId::Youdao
            } else if clipped.len() <= 720 {
                TtsEngineId::Google
            } else {
                return Ok(TtsSynthesis::Ready(TtsAudio::Browser {
                    engine: "browser".into(),
                    text: clipped,
                    lang: to_bcp47(&lang).to_owned(),
                    rate: cfg.tts_speed,
                }));
            }
        }
        TtsEngineId::Edge | TtsEngineId::Browser => unreachable!(),
    };

    let res = match chosen {
        TtsEngineId::Azure => fetch_azure_tts(&clipped, &lang, accent, cfg),
        TtsEngineId::Youdao => fetch_youdao_tts(&clipped, accent, cfg.tts_speed),
        TtsEngineId::Google => fetch_google_tts(&clipped, &lang, cfg.tts_speed),
        TtsEngineId::Openai => fetch_openai_tts(&clipped, cfg),
        TtsEngineId::Auto | TtsEngineId::Browser | TtsEngineId::Edge => unreachable!(),
    };

    match res {
        Ok(audio) => Ok(TtsSynthesis::Ready(audio)),
        Err(Error::Denied(cap)) => Err(Error::Denied(cap)),
        Err(_err) if cfg.tts_engine == TtsEngineId::Auto => {
            if chosen == TtsEngineId::Youdao
                && let Ok(g) = fetch_google_tts(&clipped, &lang, cfg.tts_speed)
            {
                return Ok(TtsSynthesis::Ready(g));
            }
            Ok(TtsSynthesis::Ready(TtsAudio::Browser {
                engine: "browser".into(),
                text: clipped,
                lang: to_bcp47(&lang).to_owned(),
                rate: cfg.tts_speed,
            }))
        }
        Err(err) => Err(err),
    }
}

pub fn start_edge_tts_ws(
    text: &str,
    lang: &str,
    accent: &str,
    cfg: &TranslationConfig,
) -> Result<TtsSynthesis, Error> {
    let projects = host::projects()?;
    let Some(project) = projects.first() else {
        return Err(Error::Invalid(
            "Edge TTS WebSocket requires an open workspace project (for host socket binding). Switch TTS engine to Azure/Youdao/Google or open a project.".into(),
        ));
    };

    let now_secs = unix_secs_now();
    let gec = compute_sec_ms_gec(now_secs);
    let conn_id = format!("{:032x}", unix_millis_now().wrapping_mul(6364136223846793005) ^ 0x9E37_79B9_7F4A_7C15);
    let url = format!(
        "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1\
         ?TrustedClientToken={EDGE_TRUSTED_CLIENT_TOKEN}\
         &Sec-MS-GEC={gec}&Sec-MS-GEC-Version={EDGE_SEC_MS_GEC_VERSION}&ConnectionId={conn_id}"
    );
    let req = WebSocketRequest {
        url,
        headers: vec![
            (
                "User-Agent".into(),
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/130.0.0.0".into(),
            ),
            ("Origin".into(), "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold".into()),
        ],
        subprotocol: String::new(),
    };
    let ws = WebSocket::connect(project, &req)?;

    let voice = select_edge_voice(&cfg.edge_voice, lang, accent);
    let ts = format!("{}Z", crate::engines::format_utc_iso8601(now_secs).trim_end_matches('Z'));
    let (config_frame, ssml_frame) = build_edge_ssml(text, &voice, cfg.tts_speed, &conn_id, &ts);
    ws.send(config_frame.as_bytes())?;
    ws.send(ssml_frame.as_bytes())?;

    let mut audio_chunks = Vec::new();
    if let Ok(batch) = ws.drain(DRAIN_LIMIT) {
        for frame in batch.messages {
            if let Some(chunk) = extract_edge_audio_chunk(&frame) {
                audio_chunks.extend_from_slice(chunk);
            }
        }
        if batch.closed {
            let _ = ws.close();
            if !audio_chunks.is_empty() {
                return Ok(TtsSynthesis::Ready(TtsAudio::DataUri {
                    engine: "edge".into(),
                    mime: "audio/mpeg".into(),
                    data_uri: format!("data:audio/mpeg;base64,{}", STANDARD.encode(&audio_chunks)),
                    rate: 1.0,
                }));
            }
            return Err(Error::Failed(
                "Edge TTS WebSocket closed without returning audio bytes".into(),
            ));
        }
    }

    Ok(TtsSynthesis::PendingEdge(PendingEdgeTts {
        ws,
        audio_chunks,
        text: text.to_owned(),
        lang: lang.to_owned(),
        accent: accent.to_owned(),
        rate: cfg.tts_speed,
        created_ms: unix_millis_now(),
    }))
}

pub fn drain_pending_edge_tts(
    pending: &mut PendingEdgeTts,
) -> Result<Option<Result<TtsAudio, Error>>, Error> {
    for _ in 0..64 {
        let batch = pending.ws.drain(DRAIN_LIMIT)?;
        let count = batch.messages.len();
        for frame in batch.messages {
            if let Some(chunk) = extract_edge_audio_chunk(&frame) {
                pending.audio_chunks.extend_from_slice(chunk);
            }
        }
        if batch.closed {
            let _ = pending.ws.close();
            if pending.audio_chunks.is_empty() {
                return Ok(Some(Err(Error::Failed(
                    "Edge TTS connection closed before audio frames arrived".into(),
                ))));
            }
            return Ok(Some(Ok(TtsAudio::DataUri {
                engine: "edge".into(),
                mime: "audio/mpeg".into(),
                data_uri: format!(
                    "data:audio/mpeg;base64,{}",
                    STANDARD.encode(&pending.audio_chunks)
                ),
                rate: 1.0,
            })));
        }
        if count < DRAIN_LIMIT as usize {
            return Ok(None);
        }
    }
    Ok(None)
}

pub fn fallback_tts(text: &str, lang: &str, accent: &str, cfg: &TranslationConfig) -> TtsAudio {
    if lang == "en"
        && text.len() <= 240
        && let Ok(yd) = fetch_youdao_tts(text, accent, cfg.tts_speed)
    {
        return yd;
    }
    if text.len() <= 720
        && let Ok(gg) = fetch_google_tts(text, lang, cfg.tts_speed)
    {
        return gg;
    }
    TtsAudio::Browser {
        engine: "browser".into(),
        text: text.to_owned(),
        lang: to_bcp47(lang).to_owned(),
        rate: cfg.tts_speed,
    }
}

fn fetch_azure_tts(
    text: &str,
    lang: &str,
    accent: &str,
    cfg: &TranslationConfig,
) -> Result<TtsAudio, Error> {
    if !host::secret_is_set("azure-speech-api-key") {
        return Err(Error::Invalid(
            "Azure Speech TTS requires secret `azure-speech-api-key` in Settings -> Plugins".into(),
        ));
    }
    let region: String = cfg
        .azure_speech_region
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase();
    let region = if region.is_empty() { "eastus".to_owned() } else { region };
    let url = format!("https://{region}.tts.speech.microsoft.com/cognitiveservices/v1");
    let voice = select_edge_voice(&cfg.azure_speech_voice, lang, accent);
    let ssml = build_azure_ssml_doc(text, &voice, cfg.tts_speed);

    let mut req = HttpRequest::post(&url, ssml)
        .header("Ocp-Apim-Subscription-Key", "{{secret:azure-speech-api-key}}")
        .header("Content-Type", "application/ssml+xml")
        .header("X-Microsoft-OutputFormat", "audio-16khz-128kbitrate-mono-mp3")
        .header("User-Agent", "openidea-translation");
    req.timeout_ms = Some(15_000);
    let resp = host::fetch(&req)?;
    if !(200..300).contains(&resp.status) || resp.body.is_empty() {
        return Err(Error::Failed(format!("Azure Speech TTS HTTP {}", resp.status)));
    }
    Ok(TtsAudio::DataUri {
        engine: "azure".into(),
        mime: "audio/mpeg".into(),
        data_uri: format!("data:audio/mpeg;base64,{}", STANDARD.encode(&resp.body)),
        rate: 1.0,
    })
}

fn fetch_youdao_tts(text: &str, accent: &str, rate: f64) -> Result<TtsAudio, Error> {
    // Youdao DictVoice: type=1 is UK, type=2 is US.
    let voice_type = if accent == "uk" { "1" } else { "2" };
    let url = format!(
        "https://dict.youdao.com/dictvoice?audio={}&type={voice_type}",
        percent_encode_rfc3986(text)
    );
    let mut req = HttpRequest::get(&url).header("Accept", "audio/mpeg,audio/*;q=0.9,*/*;q=0.8");
    req.timeout_ms = Some(10_000);
    let resp = host::fetch(&req)?;
    if !(200..300).contains(&resp.status) || resp.body.is_empty() {
        return Err(Error::Failed(format!("Youdao TTS HTTP {}", resp.status)));
    }
    Ok(TtsAudio::DataUri {
        engine: "youdao".into(),
        mime: "audio/mpeg".into(),
        data_uri: format!("data:audio/mpeg;base64,{}", STANDARD.encode(&resp.body)),
        rate,
    })
}

/// Splits long text cleanly along sentence or space boundaries for chunked Google TTS.
pub fn chunk_tts_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for word in text.split_inclusive(|c: char| c.is_whitespace() || matches!(c, '.' | ',' | ';' | '!' | '?' | '。' | '，' | '；' | '！' | '？')) {
        if !current.is_empty() && current.chars().count() + word.chars().count() > max_chars {
            let clean = current.trim().to_owned();
            if !clean.is_empty() {
                chunks.push(clean);
            }
            current.clear();
        }
        if word.chars().count() > max_chars {
            for ch in word.chars() {
                current.push(ch);
                if current.chars().count() >= max_chars {
                    chunks.push(std::mem::take(&mut current));
                }
            }
        } else {
            current.push_str(word);
        }
    }
    let rem = current.trim().to_owned();
    if !rem.is_empty() {
        chunks.push(rem);
    }
    chunks
}

fn fetch_google_tts(text: &str, lang: &str, rate: f64) -> Result<TtsAudio, Error> {
    let tl = match lang {
        "zh-CN" => "zh-CN",
        "zh-TW" => "zh-TW",
        other => other,
    };
    let chunks = chunk_tts_text(text, 180);
    let mut combined_audio = Vec::new();
    for segment in chunks.into_iter().take(6) {
        let url = format!(
            "https://translate.googleapis.com/translate_tts?ie=UTF-8&client=tw-ob&tl={}&q={}",
            percent_encode_rfc3986(tl),
            percent_encode_rfc3986(&segment)
        );
        let mut req = HttpRequest::get(&url)
            .header("Accept", "audio/mpeg,audio/*;q=0.9,*/*;q=0.8")
            .header("Referer", "https://translate.google.com/");
        req.timeout_ms = Some(10_000);
        let resp = host::fetch(&req)?;
        if !(200..300).contains(&resp.status) || resp.body.is_empty() {
            return Err(Error::Failed(format!("Google TTS HTTP {}", resp.status)));
        }
        combined_audio.extend_from_slice(&resp.body);
    }
    if combined_audio.is_empty() {
        return Err(Error::Failed("Google TTS returned empty audio".into()));
    }
    Ok(TtsAudio::DataUri {
        engine: "google".into(),
        mime: "audio/mpeg".into(),
        data_uri: format!("data:audio/mpeg;base64,{}", STANDARD.encode(&combined_audio)),
        rate,
    })
}

fn fetch_openai_tts(text: &str, cfg: &TranslationConfig) -> Result<TtsAudio, Error> {
    let base = cfg.effective_openai_tts_base_url().trim().trim_end_matches('/');
    let url = if base.ends_with("/audio/speech") {
        base.to_owned()
    } else {
        format!("{base}/audio/speech")
    };
    let secret_name = cfg.effective_tts_secret_name();
    let has_secret = host::secret_is_set(secret_name);
    if !has_secret && !is_loopback_url(&url) {
        return Err(Error::Invalid(format!(
            "OpenAI TTS requires secret `{secret_name}` in Settings -> Plugins"
        )));
    }
    let mut payload = json!({
        "model": cfg.openai_tts_model,
        "voice": cfg.openai_tts_voice,
        "input": text,
        "speed": cfg.tts_speed,
        "response_format": "mp3"
    });
    if !cfg.openai_tts_instructions.is_empty() {
        payload["instructions"] = json!(cfg.openai_tts_instructions);
    }
    let mut req = HttpRequest::post(&url, payload.to_string()).header("Content-Type", "application/json");
    if has_secret {
        req = req.header("Authorization", &format!("Bearer {{{{secret:{secret_name}}}}}"));
    }
    req.timeout_ms = Some(20_000);
    let resp = host::fetch(&req)?;
    if !(200..300).contains(&resp.status) || resp.body.is_empty() {
        return Err(Error::Failed(format!("OpenAI TTS HTTP {}", resp.status)));
    }
    Ok(TtsAudio::DataUri {
        engine: "openai".into(),
        mime: "audio/mpeg".into(),
        data_uri: format!("data:audio/mpeg;base64,{}", STANDARD.encode(&resp.body)),
        rate: 1.0,
    })
}

pub fn to_bcp47(lang: &str) -> &str {
    match lang {
        "en" => "en-US",
        "ja" => "ja-JP",
        "ko" => "ko-KR",
        "fr" => "fr-FR",
        "de" => "de-DE",
        "es" => "es-ES",
        "ru" => "ru-RU",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::HttpResponse;
    use openidea_plugin::mock::MockHost;

    #[test]
    fn edge_sec_ms_gec_and_binary_frame_parser_are_deterministic() {
        let g1 = compute_sec_ms_gec(1_700_000_100);
        let g2 = compute_sec_ms_gec(1_700_000_250);
        assert_eq!(g1.len(), 64);
        assert!(g1.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
        // Same 300s quantum -> identical token
        assert_eq!(g1, g2);

        // Simulated Edge binary frame: 2-byte header len BE + header + audio
        let hdr = b"X-RequestId:abc\r\nContent-Type:audio/mpeg\r\nPath:audio\r\n\r\n";
        let len_bytes = (hdr.len() as u16).to_be_bytes();
        let mut frame = Vec::new();
        frame.extend_from_slice(&len_bytes);
        frame.extend_from_slice(hdr);
        frame.extend_from_slice(b"MP3BYTES");
        assert_eq!(extract_edge_audio_chunk(&frame), Some(&b"MP3BYTES"[..]));
    }

    #[test]
    fn synthesizes_youdao_chunked_google_azure_and_independent_openai_tts() {
        let host = MockHost::install().with_capabilities(&[
            "storage",
            "secrets",
            "net:dict.youdao.com",
            "net:translate.googleapis.com",
            "net:api.openai.com",
            "net:eastus.tts.speech.microsoft.com",
            "net:*.tts.speech.microsoft.com",
        ]);
        host.set_secret("openai-api-key", "sk-main");
        host.set_secret("openai-tts-api-key", "sk-tts-only");
        host.set_secret("azure-speech-api-key", "az-speech-key");
        host.set_setting("openaiTtsInstructions", "\"Speak clearly\"");

        host.on_fetch(|req| {
            if req.url.contains("dict.youdao.com") {
                assert!(req.url.contains("type=1"));
                Ok(HttpResponse { status: 200, headers: vec![], body: b"MP3_YD".to_vec() })
            } else if req.url.contains("translate_tts") {
                Ok(HttpResponse { status: 200, headers: vec![], body: b"GG".to_vec() })
            } else if req.url.contains("tts.speech.microsoft.com") {
                assert!(req.headers.contains(&("Ocp-Apim-Subscription-Key".into(), "az-speech-key".into())));
                Ok(HttpResponse { status: 200, headers: vec![], body: b"MP3_AZ".to_vec() })
            } else {
                assert!(req.url.ends_with("/audio/speech"));
                assert!(req.headers.contains(&("Authorization".into(), "Bearer sk-tts-only".into())));
                let body: serde_json::Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
                assert_eq!(body["instructions"], "Speak clearly");
                Ok(HttpResponse { status: 200, headers: vec![], body: b"MP3_AI".to_vec() })
            }
        });

        let cfg = TranslationConfig::load();
        let yd = synthesize("hello", Some("en"), Some("uk"), &cfg).unwrap();
        assert!(matches!(yd, TtsAudio::DataUri { ref engine, .. } if engine == "youdao"));

        host.set_setting("ttsEngine", "\"google\"");
        let long_text = "word ".repeat(60); // > 180 chars -> multiple chunks stitched
        let gg = synthesize(&long_text, Some("en"), None, &TranslationConfig::load()).unwrap();
        assert!(matches!(gg, TtsAudio::DataUri { ref engine, ref data_uri, .. } if engine == "google" && data_uri == "data:audio/mpeg;base64,R0dHRw=="));

        host.set_setting("ttsEngine", "\"azure\"");
        let az = synthesize("你好世界", Some("zh-CN"), None, &TranslationConfig::load()).unwrap();
        assert!(matches!(az, TtsAudio::DataUri { ref engine, .. } if engine == "azure"));

        host.set_setting("ttsEngine", "\"openai\"");
        let ai = synthesize("resilient", Some("en"), None, &TranslationConfig::load()).unwrap();
        assert!(matches!(ai, TtsAudio::DataUri { ref engine, .. } if engine == "openai"));
    }

    #[test]
    fn edge_tts_websocket_drains_and_encodes_audio() {
        let host = MockHost::install().with_capabilities(&[
            "storage",
            "project.read",
            "net:speech.platform.bing.com",
        ]);
        host.set_project_root(Some("/tmp/proj"));
        host.set_setting("ttsEngine", "\"edge\"");

        let cfg = TranslationConfig::load();
        let mut pending = match synthesize_with_ws("hello world", Some("en"), Some("us"), &cfg).unwrap() {
            TtsSynthesis::PendingEdge(p) => p,
            TtsSynthesis::Ready(_) => panic!("Expected PendingEdge before server pushes frames"),
        };

        let sockets = host.websockets();
        assert_eq!(sockets.len(), 1);
        assert!(sockets[0].request.url.contains("Sec-MS-GEC="));
        assert_eq!(sockets[0].sent.len(), 2);

        let hdr = b"Path:audio\r\n\r\n";
        let mut frame = Vec::new();
        frame.extend_from_slice(&(hdr.len() as u16).to_be_bytes());
        frame.extend_from_slice(hdr);
        frame.extend_from_slice(b"EDGE_MP3");
        host.push_websocket_message(pending.ws.id(), &frame).unwrap();
        host.close_websocket(pending.ws.id()).unwrap();

        match drain_pending_edge_tts(&mut pending).unwrap().unwrap().unwrap() {
            TtsAudio::DataUri { engine, mime, data_uri, .. } => {
                assert_eq!(engine, "edge");
                assert_eq!(mime, "audio/mpeg");
                assert_eq!(data_uri, format!("data:audio/mpeg;base64,{}", STANDARD.encode(b"EDGE_MP3")));
            }
            _ => panic!("Expected DataUri from Edge TTS"),
        }
    }
}
