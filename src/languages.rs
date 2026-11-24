use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageInfo {
    pub code: &'static str,
    pub en_name: &'static str,
    pub zh_name: &'static str,
}

pub const LANGUAGES: &[LanguageInfo] = &[
    LanguageInfo { code: "auto", en_name: "Auto Detect", zh_name: "自动检测" },
    LanguageInfo { code: "zh-CN", en_name: "Chinese (Simplified)", zh_name: "简体中文" },
    LanguageInfo { code: "zh-TW", en_name: "Chinese (Traditional)", zh_name: "繁体中文" },
    LanguageInfo { code: "en", en_name: "English", zh_name: "英语" },
    LanguageInfo { code: "ja", en_name: "Japanese", zh_name: "日语" },
    LanguageInfo { code: "ko", en_name: "Korean", zh_name: "韩语" },
    LanguageInfo { code: "fr", en_name: "French", zh_name: "法语" },
    LanguageInfo { code: "de", en_name: "German", zh_name: "德语" },
    LanguageInfo { code: "es", en_name: "Spanish", zh_name: "西班牙语" },
    LanguageInfo { code: "ru", en_name: "Russian", zh_name: "俄语" },
    LanguageInfo { code: "pt", en_name: "Portuguese", zh_name: "葡萄牙语" },
    LanguageInfo { code: "it", en_name: "Italian", zh_name: "意大利语" },
    LanguageInfo { code: "nl", en_name: "Dutch", zh_name: "荷兰语" },
    LanguageInfo { code: "pl", en_name: "Polish", zh_name: "波兰语" },
    LanguageInfo { code: "tr", en_name: "Turkish", zh_name: "土耳其语" },
    LanguageInfo { code: "vi", en_name: "Vietnamese", zh_name: "越南语" },
    LanguageInfo { code: "th", en_name: "Thai", zh_name: "泰语" },
    LanguageInfo { code: "id", en_name: "Indonesian", zh_name: "印度尼西亚语" },
    LanguageInfo { code: "ar", en_name: "Arabic", zh_name: "阿拉伯语" },
    LanguageInfo { code: "hi", en_name: "Hindi", zh_name: "印地语" },
    LanguageInfo { code: "uk", en_name: "Ukrainian", zh_name: "乌克兰语" },
    LanguageInfo { code: "cs", en_name: "Czech", zh_name: "捷克语" },
    LanguageInfo { code: "sv", en_name: "Swedish", zh_name: "瑞典语" },
    LanguageInfo { code: "da", en_name: "Danish", zh_name: "丹麦语" },
    LanguageInfo { code: "fi", en_name: "Finnish", zh_name: "芬兰语" },
    LanguageInfo { code: "no", en_name: "Norwegian", zh_name: "挪威语" },
    LanguageInfo { code: "el", en_name: "Greek", zh_name: "希腊语" },
    LanguageInfo { code: "he", en_name: "Hebrew", zh_name: "希伯来语" },
    LanguageInfo { code: "ro", en_name: "Romanian", zh_name: "罗马尼亚语" },
    LanguageInfo { code: "hu", en_name: "Hungarian", zh_name: "匈牙利语" },
    LanguageInfo { code: "bg", en_name: "Bulgarian", zh_name: "保加利亚语" },
    LanguageInfo { code: "sk", en_name: "Slovak", zh_name: "斯洛伐克语" },
    LanguageInfo { code: "ms", en_name: "Malay", zh_name: "马来语" },
    LanguageInfo { code: "fa", en_name: "Persian", zh_name: "波斯语" },
    LanguageInfo { code: "ta", en_name: "Tamil", zh_name: "泰米尔语" },
    LanguageInfo { code: "la", en_name: "Latin", zh_name: "拉丁语" },
];

pub fn is_valid_language_code(code: &str) -> bool {
    LANGUAGES.iter().any(|l| l.code == code)
}

pub fn normalize_lang_pref(raw: Option<&str>, allow_auto: bool, default: &str) -> String {
    let trimmed = raw.unwrap_or("").trim();
    if trimmed.is_empty() {
        return default.to_owned();
    }
    if trimmed.eq_ignore_ascii_case("auto") {
        return if allow_auto { "auto".to_owned() } else { default.to_owned() };
    }
    for info in LANGUAGES {
        if info.code.eq_ignore_ascii_case(trimmed) {
            if info.code == "auto" && !allow_auto {
                return default.to_owned();
            }
            return info.code.to_owned();
        }
    }
    normalize_detected_code(trimmed)
}

pub fn display_name(code: &str, zh: bool) -> &'static str {
    for info in LANGUAGES {
        if info.code.eq_ignore_ascii_case(code) {
            return if zh { info.zh_name } else { info.en_name };
        }
    }
    if zh { "未知语言" } else { "Unknown" }
}

/// Fast Unicode-script detector used for smart Primary <-> Secondary switching
/// and local language hints.
pub fn detect_script_language(text: &str) -> &'static str {
    let mut kana = 0usize;
    let mut hangul = 0usize;
    let mut han = 0usize;
    let mut cyrillic = 0usize;
    let mut arabic = 0usize;
    let mut hebrew = 0usize;
    let mut greek = 0usize;
    let mut thai = 0usize;
    let mut devanagari = 0usize;
    let mut tamil = 0usize;
    let mut latin = 0usize;

    for ch in text.chars() {
        match ch as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF => kana += 1,
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => hangul += 1,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF => han += 1,
            0x0400..=0x04FF => cyrillic += 1,
            0x0600..=0x06FF | 0x0750..=0x077F => arabic += 1,
            0x0590..=0x05FF => hebrew += 1,
            0x0370..=0x03FF => greek += 1,
            0x0E00..=0x0E7F => thai += 1,
            0x0900..=0x097F => devanagari += 1,
            0x0B80..=0x0BFF => tamil += 1,
            _ if ch.is_ascii_alphabetic() || (0x00C0..=0x024F).contains(&(ch as u32)) => latin += 1,
            _ => {}
        }
    }

    if kana > 0 {
        return "ja";
    }
    if hangul > 0 {
        return "ko";
    }
    if han > 0 && han * 2 >= latin {
        return "zh-CN";
    }
    let max_other = [
        (cyrillic, "ru"),
        (arabic, "ar"),
        (hebrew, "he"),
        (greek, "el"),
        (thai, "th"),
        (devanagari, "hi"),
        (tamil, "ta"),
    ]
    .into_iter()
    .max_by_key(|(count, _)| *count)
    .unwrap_or((0, "en"));

    if max_other.0 > latin && max_other.0 > 0 {
        return max_other.1;
    }
    if han > 0 {
        return "zh-CN";
    }
    "en"
}

fn lang_family(code: &str) -> &str {
    let lower = code.trim();
    if lower.eq_ignore_ascii_case("zh")
        || lower.eq_ignore_ascii_case("zh-cn")
        || lower.eq_ignore_ascii_case("zh-tw")
        || lower.eq_ignore_ascii_case("zh-chs")
        || lower.eq_ignore_ascii_case("zh-cht")
        || lower.eq_ignore_ascii_case("zh-hans")
        || lower.eq_ignore_ascii_case("zh-hant")
    {
        "zh"
    } else if let Some((head, _)) = lower.split_once('-') {
        head
    } else {
        lower
    }
}

fn same_language_family(a: &str, b: &str) -> bool {
    lang_family(a).eq_ignore_ascii_case(lang_family(b))
}

/// Resolves `(source, target)` using explicit preferences or smart
/// `Primary <-> Secondary` switching when `target_pref == "auto"`.
pub fn resolve_lang_pair(
    text: &str,
    source_pref: &str,
    target_pref: &str,
    primary: &str,
    secondary: &str,
) -> (String, String) {
    resolve_lang_pair_with_strategy(text, source_pref, target_pref, primary, secondary, "primary", None)
}

pub fn resolve_lang_pair_with_strategy(
    text: &str,
    source_pref: &str,
    target_pref: &str,
    primary: &str,
    secondary: &str,
    strategy: &str,
    last_target: Option<&str>,
) -> (String, String) {
    let src = if source_pref.trim().is_empty() {
        "auto".to_owned()
    } else {
        source_pref.to_owned()
    };

    if target_pref != "auto" && !target_pref.trim().is_empty() {
        return (src, target_pref.to_owned());
    }

    let hint = if src != "auto" {
        src.as_str()
    } else {
        detect_script_language(text)
    };
    if strategy.eq_ignore_ascii_case("last")
        && let Some(last) = last_target
        && !last.trim().is_empty()
        && last != "auto"
        && !same_language_family(hint, last)
    {
        return (src, last.to_owned());
    }
    let target = if same_language_family(hint, primary) {
        secondary.to_owned()
    } else {
        primary.to_owned()
    };
    (src, target)
}

pub fn normalize_detected_code(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "auto".to_owned();
    }
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "zh" | "zh-cn" | "zh-chs" | "zh-hans" | "zh_cn" => "zh-CN".to_owned(),
        "zh-tw" | "zh-hk" | "zh-cht" | "zh-hant" | "zh_tw" | "cht" => "zh-TW".to_owned(),
        "en" | "en-us" | "en-gb" => "en".to_owned(),
        "ja" | "jp" => "ja".to_owned(),
        "ko" | "kor" => "ko".to_owned(),
        "fr" | "fra" => "fr".to_owned(),
        "de" | "deu" => "de".to_owned(),
        "es" | "spa" => "es".to_owned(),
        "ru" | "rus" => "ru".to_owned(),
        "pt" | "pt-pt" | "pt-br" => "pt".to_owned(),
        "it" | "ita" => "it".to_owned(),
        "nl" => "nl".to_owned(),
        "pl" => "pl".to_owned(),
        "tr" => "tr".to_owned(),
        "vi" | "vie" => "vi".to_owned(),
        "th" => "th".to_owned(),
        "id" => "id".to_owned(),
        "ar" | "ara" => "ar".to_owned(),
        "hi" => "hi".to_owned(),
        "uk" => "uk".to_owned(),
        "cs" => "cs".to_owned(),
        "sv" | "swe" => "sv".to_owned(),
        "da" | "dan" => "da".to_owned(),
        "fi" | "fin" => "fi".to_owned(),
        "no" | "nb" => "no".to_owned(),
        "el" => "el".to_owned(),
        "he" | "iw" => "he".to_owned(),
        "ro" | "rom" => "ro".to_owned(),
        "hu" => "hu".to_owned(),
        "bg" | "bul" => "bg".to_owned(),
        "sk" => "sk".to_owned(),
        "ms" => "ms".to_owned(),
        "fa" => "fa".to_owned(),
        "ta" => "ta".to_owned(),
        "la" => "la".to_owned(),
        _ => trimmed.to_owned(),
    }
}

pub fn to_google_code(code: &str) -> &str {
    match code {
        "auto" => "auto",
        "zh-CN" => "zh-CN",
        "zh-TW" => "zh-TW",
        "he" => "he",
        other => other,
    }
}

pub fn to_microsoft_code(code: &str) -> Option<&str> {
    match code {
        "auto" => None,
        "zh-CN" => Some("zh-Hans"),
        "zh-TW" => Some("zh-Hant"),
        "no" => Some("nb"),
        other => Some(other),
    }
}

pub fn to_deepl_source(code: &str) -> Option<&'static str> {
    match code {
        "auto" => None,
        "zh-CN" | "zh-TW" => Some("ZH"),
        "en" => Some("EN"),
        "ja" => Some("JA"),
        "ko" => Some("KO"),
        "fr" => Some("FR"),
        "de" => Some("DE"),
        "es" => Some("ES"),
        "ru" => Some("RU"),
        "pt" => Some("PT"),
        "it" => Some("IT"),
        "nl" => Some("NL"),
        "pl" => Some("PL"),
        "tr" => Some("TR"),
        "id" => Some("ID"),
        "ar" => Some("AR"),
        "uk" => Some("UK"),
        "cs" => Some("CS"),
        "sv" => Some("SV"),
        "da" => Some("DA"),
        "fi" => Some("FI"),
        "no" => Some("NB"),
        "el" => Some("EL"),
        "he" => Some("HE"),
        "ro" => Some("RO"),
        "hu" => Some("HU"),
        "bg" => Some("BG"),
        "sk" => Some("SK"),
        _ => None,
    }
}

pub fn to_deepl_target(code: &str) -> &'static str {
    match code {
        "zh-CN" => "ZH-HANS",
        "zh-TW" => "ZH-HANT",
        "en" => "EN-US",
        "ja" => "JA",
        "ko" => "KO",
        "fr" => "FR",
        "de" => "DE",
        "es" => "ES",
        "ru" => "RU",
        "pt" => "PT-PT",
        "it" => "IT",
        "nl" => "NL",
        "pl" => "PL",
        "tr" => "TR",
        "id" => "ID",
        "ar" => "AR",
        "uk" => "UK",
        "cs" => "CS",
        "sv" => "SV",
        "da" => "DA",
        "fi" => "FI",
        "no" => "NB",
        "el" => "EL",
        "he" => "HE",
        "ro" => "RO",
        "hu" => "HU",
        "bg" => "BG",
        "sk" => "SK",
        "vi" => "VI",
        "th" => "TH",
        _ => "EN-US",
    }
}

pub fn deepl_supports_formality(target: &str) -> bool {
    matches!(
        target,
        "DE" | "FR" | "IT" | "ES" | "NL" | "PL" | "PT-BR" | "PT-PT" | "JA" | "RU"
    )
}

pub fn to_youdao_code(code: &str) -> &str {
    match code {
        "auto" => "auto",
        "zh-CN" => "zh-CHS",
        "zh-TW" => "zh-CHT",
        other => other,
    }
}

pub fn to_baidu_code(code: &str) -> &str {
    match code {
        "auto" => "auto",
        "zh-CN" => "zh",
        "zh-TW" => "cht",
        "ja" => "jp",
        "ko" => "kor",
        "fr" => "fra",
        "es" => "spa",
        "ar" => "ara",
        "vi" => "vie",
        "da" => "dan",
        "fi" => "fin",
        "ro" => "rom",
        "bg" => "bul",
        "sv" => "swe",
        "no" => "nor",
        other => other,
    }
}

pub fn to_alibaba_code(code: &str) -> &str {
    match code {
        "auto" => "auto",
        "zh-CN" => "zh",
        "zh-TW" => "zh-tw",
        other => other,
    }
}

pub fn to_openai_name(code: &str) -> String {
    match code {
        "auto" => "auto-detected source language".into(),
        "zh-CN" => "Simplified Chinese (zh-CN)".into(),
        "zh-TW" => "Traditional Chinese (zh-TW)".into(),
        _ => format!("{} ({code})", display_name(code, false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_scripts_accurately() {
        assert_eq!(detect_script_language("Hello world"), "en");
        assert_eq!(detect_script_language("用户认证令牌"), "zh-CN");
        assert_eq!(detect_script_language("ユーザー設定を保存します"), "ja");
        assert_eq!(detect_script_language("사용자 프로필"), "ko");
        assert_eq!(detect_script_language("Привет мир"), "ru");
        assert_eq!(detect_script_language("مرحبا بالعالم"), "ar");
    }

    #[test]
    fn smart_primary_secondary_switching() {
        assert_eq!(
            resolve_lang_pair("getUserProfile", "auto", "auto", "zh-CN", "en"),
            ("auto".into(), "zh-CN".into())
        );
        assert_eq!(
            resolve_lang_pair("获取用户信息", "auto", "auto", "zh-CN", "en"),
            ("auto".into(), "en".into())
        );
        // Traditional Chinese input also flips when primary is Simplified Chinese
        assert_eq!(
            resolve_lang_pair("獲取用戶資訊", "auto", "auto", "zh-CN", "en"),
            ("auto".into(), "en".into())
        );
        // Explicit target overrides smart switching
        assert_eq!(
            resolve_lang_pair("hello", "en", "ja", "zh-CN", "en"),
            ("en".into(), "ja".into())
        );
    }

    #[test]
    fn maps_engine_language_codes() {
        assert_eq!(to_microsoft_code("zh-CN"), Some("zh-Hans"));
        assert_eq!(to_microsoft_code("zh-TW"), Some("zh-Hant"));
        assert_eq!(to_microsoft_code("auto"), None);
        assert_eq!(to_deepl_source("zh-CN"), Some("ZH"));
        assert_eq!(to_deepl_target("zh-TW"), "ZH-HANT");
        assert_eq!(to_deepl_target("en"), "EN-US");
        assert_eq!(to_youdao_code("zh-CN"), "zh-CHS");
        assert_eq!(to_baidu_code("ja"), "jp");
        assert_eq!(to_baidu_code("ko"), "kor");
        assert_eq!(to_alibaba_code("zh-CN"), "zh");
        assert_eq!(to_alibaba_code("zh-TW"), "zh-tw");
    }
}
