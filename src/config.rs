use crate::languages::normalize_lang_pref;
use openidea_plugin::Error;
use openidea_plugin::host::{self, storage};
use serde::{Deserialize, Serialize};

pub const OVERRIDE_ENGINE: &str = "override:engine";
pub const OVERRIDE_SOURCE_LANG: &str = "override:sourceLang";
pub const OVERRIDE_TARGET_LANG: &str = "override:targetLang";
pub const OVERRIDE_AUTO_SPLIT: &str = "override:autoSplitWords";
pub const OVERRIDE_INLINE_DOC: &str = "override:showDocAnnotations";
pub const OVERRIDE_INSTANT: &str = "override:instantTranslation";
pub const OVERRIDE_DOC_BILINGUAL: &str = "override:docBilingualMode";
pub const OVERRIDE_TTS_ENGINE: &str = "override:ttsEngine";
pub const OVERRIDE_EDGE_VOICE: &str = "override:edgeVoice";
pub const STATE_LAST_TARGET_LANG: &str = "state:lastTargetLang";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineId {
    Google,
    Microsoft,
    Openai,
    Deepl,
    Youdao,
    Baidu,
    Alibaba,
}

impl EngineId {
    pub const ALL: [Self; 7] = [
        Self::Google,
        Self::Microsoft,
        Self::Openai,
        Self::Deepl,
        Self::Youdao,
        Self::Baidu,
        Self::Alibaba,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "google" => Some(Self::Google),
            "microsoft" => Some(Self::Microsoft),
            "openai" => Some(Self::Openai),
            "deepl" => Some(Self::Deepl),
            "youdao" => Some(Self::Youdao),
            "baidu" => Some(Self::Baidu),
            "alibaba" => Some(Self::Alibaba),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Microsoft => "microsoft",
            Self::Openai => "openai",
            Self::Deepl => "deepl",
            Self::Youdao => "youdao",
            Self::Baidu => "baidu",
            Self::Alibaba => "alibaba",
        }
    }

    pub fn display_name(self, zh: bool) -> &'static str {
        match (self, zh) {
            (Self::Google, false) => "Google Translate",
            (Self::Google, true) => "Google 翻译",
            (Self::Microsoft, false) => "Microsoft Translator",
            (Self::Microsoft, true) => "微软翻译",
            (Self::Openai, false) => "OpenAI / Compatible LLM",
            (Self::Openai, true) => "OpenAI / 兼容大模型",
            (Self::Deepl, false) => "DeepL",
            (Self::Deepl, true) => "DeepL 翻译",
            (Self::Youdao, false) => "Youdao Translate",
            (Self::Youdao, true) => "有道智云翻译",
            (Self::Baidu, false) => "Baidu Translate",
            (Self::Baidu, true) => "百度翻译",
            (Self::Alibaba, false) => "Alibaba Cloud MT",
            (Self::Alibaba, true) => "阿里云机器翻译",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::Microsoft => "Microsoft",
            Self::Openai => "OpenAI",
            Self::Deepl => "DeepL",
            Self::Youdao => "Youdao",
            Self::Baidu => "Baidu",
            Self::Alibaba => "Alibaba",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsEngineId {
    Auto,
    Edge,
    Azure,
    Youdao,
    Google,
    Openai,
    Browser,
}

impl TtsEngineId {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "edge" => Some(Self::Edge),
            "azure" => Some(Self::Azure),
            "youdao" => Some(Self::Youdao),
            "google" => Some(Self::Google),
            "openai" => Some(Self::Openai),
            "browser" => Some(Self::Browser),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Edge => "edge",
            Self::Azure => "azure",
            Self::Youdao => "youdao",
            Self::Google => "google",
            Self::Openai => "openai",
            Self::Browser => "browser",
        }
    }
}

/// Full runtime configuration loaded from plugin settings + quick overrides.
/// Secret fields are read directly inside `engines.rs` / `tts.rs` and are never
/// included in any `ConfigSnapshot` sent to the WebView.
#[derive(Clone, Debug)]
pub struct TranslationConfig {
    pub ui_language: String,
    pub effective_locale: String,
    pub engine: EngineId,
    pub instant_translation: bool,
    pub primary_language: String,
    pub secondary_language: String,
    pub source_language: String,
    pub target_language: String,
    pub target_language_selection: String,
    pub auto_selection_mode: String,
    pub auto_split_words: bool,
    pub separators: String,
    pub ignore_regexp: String,
    pub show_hover_translation: bool,
    pub show_doc_annotations: bool,
    pub preferred_naming_style: String,
    pub cache_enabled: bool,
    pub cache_ttl_hours: u64,
    pub max_history_items: usize,
    pub show_words_on_startup: bool,

    pub keep_format: bool,
    pub strip_articles_in_identifier: bool,
    pub auto_replace: bool,
    pub select_target_language_before_replacement: bool,
    pub doc_bilingual_mode: bool,
    pub fold_original_in_doc: bool,
    pub open_doc_preview_tab: bool,
    pub preserve_inline_tags_in_doc: bool,

    pub openai_base_url: String,
    pub openai_model: String,
    pub openai_temperature: f64,
    pub openai_structured_dict: bool,
    pub openai_system_prompt: String,
    pub openai_translate_prompt: String,
    pub openai_document_prompt: String,

    pub deepl_tier: String,
    pub deepl_custom_url: String,
    pub deepl_formality: String,

    pub microsoft_mode: String,
    pub microsoft_region: String,
    pub microsoft_custom_endpoint: String,

    pub google_mode: String,
    pub google_custom_host: String,
    pub google_use_spell_check: bool,

    pub youdao_app_key: String,
    pub youdao_app_secret: String,

    pub baidu_app_id: String,
    pub baidu_secret_key: String,

    pub alibaba_access_key_id: String,
    pub alibaba_access_key_secret: String,
    pub alibaba_region_id: String,
    pub alibaba_scene: String,

    pub tts_engine: TtsEngineId,
    pub tts_accent: String,
    pub edge_voice: String,
    pub azure_speech_region: String,
    pub azure_speech_voice: String,
    pub openai_tts_base_url: String,
    pub openai_tts_model: String,
    pub openai_tts_voice: String,
    pub openai_tts_instructions: String,
    pub tts_speed: f64,
    pub tts_auto_play: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatusEntry {
    pub id: &'static str,
    pub name: String,
    pub configured: bool,
    pub mode: String,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsSecretStatus {
    pub openai_tts_ready: bool,
    pub azure_speech_ready: bool,
    pub has_separate_openai_tts_key: bool,
}

/// Sanitized configuration snapshot serialized to the WebView UI.
/// Contains ZERO API secrets or signing keys.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSnapshot {
    pub effective_locale: String,
    pub is_zh: bool,
    pub engine: String,
    pub instant_translation: bool,
    pub primary_language: String,
    pub secondary_language: String,
    pub source_language: String,
    pub target_language: String,
    pub target_language_selection: String,
    pub auto_selection_mode: String,
    pub auto_split_words: bool,
    pub separators: String,
    pub ignore_regexp: String,
    pub show_hover_translation: bool,
    pub show_doc_annotations: bool,
    pub preferred_naming_style: String,
    pub cache_enabled: bool,
    pub cache_ttl_hours: u64,
    pub max_history_items: usize,
    pub show_words_on_startup: bool,
    pub keep_format: bool,
    pub strip_articles_in_identifier: bool,
    pub auto_replace: bool,
    pub select_target_language_before_replacement: bool,
    pub doc_bilingual_mode: bool,
    pub fold_original_in_doc: bool,
    pub open_doc_preview_tab: bool,
    pub preserve_inline_tags_in_doc: bool,
    pub tts_engine: String,
    pub tts_accent: String,
    pub edge_voice: String,
    pub azure_speech_region: String,
    pub tts_speed: f64,
    pub tts_auto_play: bool,
    pub tts_secrets_status: TtsSecretStatus,
    pub engines: Vec<EngineStatusEntry>,
}

fn storage_string(key: &str) -> Option<String> {
    storage::get(key).ok().flatten().filter(|v| !v.trim().is_empty())
}

fn storage_bool(key: &str) -> Option<bool> {
    match storage::get(key).ok().flatten().as_deref() {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

fn setting_string_nonempty(key: &str, default: &str) -> String {
    host::setting_string(key)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn setting_enum(key: &str, allowed: &[&str], default: &str) -> String {
    host::setting_string(key)
        .map(|s| s.trim().to_owned())
        .filter(|s| allowed.contains(&s.as_str()))
        .unwrap_or_else(|| default.to_owned())
}

pub fn is_zh_locale(locale: &str) -> bool {
    locale.trim().to_ascii_lowercase().starts_with("zh")
}

impl TranslationConfig {
    pub fn load() -> Self {
        let ide_locale = {
            let raw = host::locale();
            if raw.trim().is_empty() { "en".to_owned() } else { raw }
        };
        let ui_language = setting_enum("uiLanguage", &["auto", "en", "zh-CN"], "auto");
        let effective_locale = if ui_language == "auto" {
            ide_locale
        } else {
            ui_language.clone()
        };

        let engine = storage_string(OVERRIDE_ENGINE)
            .and_then(|s| EngineId::parse(&s))
            .or_else(|| host::setting_string("engine").and_then(|s| EngineId::parse(&s)))
            .unwrap_or(EngineId::Microsoft);
        let instant_translation = storage_bool(OVERRIDE_INSTANT)
            .unwrap_or_else(|| host::setting_bool("instantTranslation").unwrap_or(true));

        let primary_language =
            normalize_lang_pref(host::setting_string("primaryLanguage").as_deref(), false, "zh-CN");
        let secondary_language =
            normalize_lang_pref(host::setting_string("secondaryLanguage").as_deref(), false, "en");

        let source_language = normalize_lang_pref(
            storage_string(OVERRIDE_SOURCE_LANG)
                .or_else(|| host::setting_string("sourceLanguage"))
                .as_deref(),
            true,
            "auto",
        );
        let target_language = normalize_lang_pref(
            storage_string(OVERRIDE_TARGET_LANG)
                .or_else(|| host::setting_string("targetLanguage"))
                .as_deref(),
            true,
            "auto",
        );
        let target_language_selection = setting_enum(
            "targetLanguageSelection",
            &["primary", "last", "default"],
            "primary",
        );

        let auto_selection_mode =
            setting_enum("autoSelectionMode", &["inclusive", "exclusive"], "inclusive");
        let auto_split_words = storage_bool(OVERRIDE_AUTO_SPLIT)
            .unwrap_or_else(|| host::setting_bool("autoSplitWords").unwrap_or(true));
        let separators = host::setting_string("separators")
            .unwrap_or_else(|| "_-.".to_owned());
        let ignore_regexp = host::setting_string("ignoreRegExp")
            .unwrap_or_default()
            .trim()
            .to_owned();
        let show_hover_translation = host::setting_bool("showHoverTranslation").unwrap_or(true);
        let show_doc_annotations = storage_bool(OVERRIDE_INLINE_DOC)
            .unwrap_or_else(|| host::setting_bool("showDocAnnotations").unwrap_or(true));
        let preferred_naming_style = setting_enum(
            "preferredNamingStyle",
            &[
                "camelCase",
                "PascalCase",
                "snake_case",
                "SCREAMING_SNAKE_CASE",
                "kebab-case",
                "dot.case",
                "Title Case",
                "UPPER CASE",
                "lower case",
                "original",
            ],
            "camelCase",
        );
        let cache_enabled = host::setting_bool("cacheEnabled").unwrap_or(true);
        let cache_ttl_hours = host::setting_number("cacheTtlHours")
            .filter(|v| v.is_finite())
            .map_or(168, |n| (n.round() as i64).clamp(0, 87600) as u64);
        let max_history_items = host::setting_number("maxHistoryItems")
            .filter(|v| v.is_finite())
            .map_or(100, |n| (n.round() as i64).clamp(10, 500) as usize);
        let show_words_on_startup = host::setting_bool("showWordsOnStartup").unwrap_or(false);

        let keep_format = host::setting_bool("keepFormat").unwrap_or(true);
        let strip_articles_in_identifier =
            host::setting_bool("stripArticlesInIdentifier").unwrap_or(true);
        let auto_replace = host::setting_bool("autoReplace").unwrap_or(false);
        let select_target_language_before_replacement =
            host::setting_bool("selectTargetLanguageBeforeReplacement").unwrap_or(false);
        let doc_bilingual_mode = storage_bool(OVERRIDE_DOC_BILINGUAL)
            .unwrap_or_else(|| host::setting_bool("docBilingualMode").unwrap_or(false));
        let fold_original_in_doc = host::setting_bool("foldOriginalInDoc").unwrap_or(false);
        let open_doc_preview_tab = host::setting_bool("openDocPreviewTab").unwrap_or(true);
        let preserve_inline_tags_in_doc =
            host::setting_bool("preserveInlineTagsInDoc").unwrap_or(true);

        let openai_base_url =
            setting_string_nonempty("openaiBaseUrl", "https://api.openai.com/v1");
        let openai_model = setting_string_nonempty("openaiModel", "gpt-4o-mini");
        let openai_temperature = host::setting_number("openaiTemperature")
            .filter(|v| v.is_finite())
            .map_or(0.2, |v| v.clamp(0.0, 2.0));
        let openai_structured_dict = host::setting_bool("openaiStructuredDict").unwrap_or(true);
        let openai_system_prompt = host::setting_string("openaiSystemPrompt").unwrap_or_default();
        let openai_translate_prompt =
            host::setting_string("openaiTranslatePrompt").unwrap_or_default();
        let openai_document_prompt =
            host::setting_string("openaiDocumentPrompt").unwrap_or_default();

        let deepl_tier = setting_enum("deeplTier", &["free", "pro", "custom"], "free");
        let deepl_custom_url =
            host::setting_string("deeplCustomUrl").unwrap_or_default().trim().to_owned();
        let deepl_formality = setting_enum(
            "deeplFormality",
            &["default", "more", "less", "prefer_more", "prefer_less"],
            "default",
        );

        let microsoft_mode = setting_enum("microsoftMode", &["edge-free", "azure"], "edge-free");
        let microsoft_region = setting_string_nonempty("microsoftRegion", "global");
        let microsoft_custom_endpoint = host::setting_string("microsoftCustomEndpoint")
            .unwrap_or_default()
            .trim()
            .to_owned();

        let google_mode = setting_enum("googleMode", &["free", "cloud-api"], "free");
        let google_custom_host =
            host::setting_string("googleCustomHost").unwrap_or_default().trim().to_owned();
        let google_use_spell_check = host::setting_bool("googleUseSpellCheck").unwrap_or(true);

        let youdao_app_key = host::setting_string("youdaoAppKey").unwrap_or_default().trim().to_owned();
        let youdao_app_secret =
            host::setting_string("youdaoAppSecret").unwrap_or_default().trim().to_owned();

        let baidu_app_id = host::setting_string("baiduAppId").unwrap_or_default().trim().to_owned();
        let baidu_secret_key =
            host::setting_string("baiduSecretKey").unwrap_or_default().trim().to_owned();

        let alibaba_access_key_id = host::setting_string("alibabaAccessKeyId")
            .unwrap_or_default()
            .trim()
            .to_owned();
        let alibaba_access_key_secret = host::setting_string("alibabaAccessKeySecret")
            .unwrap_or_default()
            .trim()
            .to_owned();
        let alibaba_region_id = setting_string_nonempty("alibabaRegionId", "cn-hangzhou");
        let alibaba_scene = setting_enum(
            "alibabaScene",
            &[
                "general",
                "title",
                "description",
                "communication",
                "medical",
                "social",
                "finance",
            ],
            "general",
        );

        let tts_engine = storage_string(OVERRIDE_TTS_ENGINE)
            .and_then(|s| TtsEngineId::parse(&s))
            .or_else(|| host::setting_string("ttsEngine").and_then(|s| TtsEngineId::parse(&s)))
            .unwrap_or(TtsEngineId::Auto);
        let tts_accent = setting_enum("ttsAccent", &["us", "uk"], "us");
        let edge_voice = storage_string(OVERRIDE_EDGE_VOICE)
            .unwrap_or_else(|| setting_string_nonempty("edgeVoice", "auto"));
        let azure_speech_region = setting_string_nonempty("azureSpeechRegion", "eastus");
        let azure_speech_voice = setting_string_nonempty("azureSpeechVoice", "auto");
        let openai_tts_base_url = host::setting_string("openaiTtsBaseUrl")
            .unwrap_or_default()
            .trim()
            .to_owned();
        let openai_tts_model = setting_enum(
            "openaiTtsModel",
            &["gpt-4o-mini-tts", "tts-1", "tts-1-hd"],
            "gpt-4o-mini-tts",
        );
        let openai_tts_voice = setting_enum(
            "openaiTtsVoice",
            &[
                "alloy", "ash", "ballad", "coral", "echo", "fable", "marin", "cedar", "nova",
                "onyx", "sage", "shimmer", "verse",
            ],
            "alloy",
        );
        let openai_tts_instructions = host::setting_string("openaiTtsInstructions")
            .unwrap_or_default()
            .trim()
            .to_owned();
        let tts_speed = host::setting_number("ttsSpeed")
            .filter(|v| v.is_finite())
            .map_or(1.0, |v| v.clamp(0.5, 2.0));
        let tts_auto_play = host::setting_bool("ttsAutoPlay").unwrap_or(false);

        Self {
            ui_language,
            effective_locale,
            engine,
            instant_translation,
            primary_language,
            secondary_language,
            source_language,
            target_language,
            target_language_selection,
            auto_selection_mode,
            auto_split_words,
            separators,
            ignore_regexp,
            show_hover_translation,
            show_doc_annotations,
            preferred_naming_style,
            cache_enabled,
            cache_ttl_hours,
            max_history_items,
            show_words_on_startup,
            keep_format,
            strip_articles_in_identifier,
            auto_replace,
            select_target_language_before_replacement,
            doc_bilingual_mode,
            fold_original_in_doc,
            open_doc_preview_tab,
            preserve_inline_tags_in_doc,
            openai_base_url,
            openai_model,
            openai_temperature,
            openai_structured_dict,
            openai_system_prompt,
            openai_translate_prompt,
            openai_document_prompt,
            deepl_tier,
            deepl_custom_url,
            deepl_formality,
            microsoft_mode,
            microsoft_region,
            microsoft_custom_endpoint,
            google_mode,
            google_custom_host,
            google_use_spell_check,
            youdao_app_key,
            youdao_app_secret,
            baidu_app_id,
            baidu_secret_key,
            alibaba_access_key_id,
            alibaba_access_key_secret,
            alibaba_region_id,
            alibaba_scene,
            tts_engine,
            tts_accent,
            edge_voice,
            azure_speech_region,
            azure_speech_voice,
            openai_tts_base_url,
            openai_tts_model,
            openai_tts_voice,
            openai_tts_instructions,
            tts_speed,
            tts_auto_play,
        }
    }

    pub fn is_zh(&self) -> bool {
        is_zh_locale(&self.effective_locale)
    }

    pub fn effective_openai_tts_base_url(&self) -> &str {
        if self.openai_tts_base_url.trim().is_empty() {
            &self.openai_base_url
        } else {
            self.openai_tts_base_url.trim()
        }
    }

    pub fn effective_tts_secret_name(&self) -> &'static str {
        if host::secret_is_set("openai-tts-api-key") {
            "openai-tts-api-key"
        } else {
            "openai-api-key"
        }
    }

    pub fn snapshot(&self) -> ConfigSnapshot {
        let zh = self.is_zh();
        let openai_local = is_loopback_url(&self.openai_base_url);
        let openai_tts_local = is_loopback_url(self.effective_openai_tts_base_url());
        let openai_key = host::secret_is_set("openai-api-key");
        let openai_tts_key = host::secret_is_set("openai-tts-api-key");
        let deepl_key = host::secret_is_set("deepl-api-key");
        let ms_key = host::secret_is_set("microsoft-api-key");
        let google_key = host::secret_is_set("google-api-key");
        let azure_speech_key = host::secret_is_set("azure-speech-api-key");

        let engines = vec![
            EngineStatusEntry {
                id: "google",
                name: EngineId::Google.display_name(zh).to_owned(),
                configured: self.google_mode == "free" || google_key,
                mode: self.google_mode.clone(),
                note: if self.google_mode == "free" {
                    if zh { "免密钥 · 含词典释义" } else { "No key · Rich dictionary" }.to_owned()
                } else if google_key {
                    if zh { "Cloud API 密钥已配置" } else { "Cloud API key ready" }.to_owned()
                } else {
                    if zh { "缺少 google-api-key 密钥" } else { "Needs google-api-key secret" }.to_owned()
                },
            },
            EngineStatusEntry {
                id: "microsoft",
                name: EngineId::Microsoft.display_name(zh).to_owned(),
                configured: self.microsoft_mode == "edge-free" || ms_key,
                mode: self.microsoft_mode.clone(),
                note: if self.microsoft_mode == "edge-free" {
                    if zh { "Edge 免密钥模式" } else { "Edge free mode (no key)" }.to_owned()
                } else if ms_key {
                    format!("Azure ({})", self.microsoft_region)
                } else {
                    if zh { "缺少 microsoft-api-key 密钥" } else { "Needs microsoft-api-key secret" }.to_owned()
                },
            },
            EngineStatusEntry {
                id: "openai",
                name: EngineId::Openai.display_name(zh).to_owned(),
                configured: openai_key || openai_local,
                mode: self.openai_model.clone(),
                note: if openai_key {
                    format!("{} · {}", self.openai_model, host_display(&self.openai_base_url))
                } else if openai_local {
                    format!("{} · local", self.openai_model)
                } else {
                    if zh { "缺少 openai-api-key 密钥" } else { "Needs openai-api-key secret" }.to_owned()
                },
            },
            EngineStatusEntry {
                id: "deepl",
                name: EngineId::Deepl.display_name(zh).to_owned(),
                configured: deepl_key,
                mode: self.deepl_tier.clone(),
                note: if deepl_key {
                    format!("{} · {}", self.deepl_tier, if zh { "密钥已就绪" } else { "Key ready" })
                } else {
                    if zh { "缺少 deepl-api-key 密钥" } else { "Needs deepl-api-key secret" }.to_owned()
                },
            },
            EngineStatusEntry {
                id: "youdao",
                name: EngineId::Youdao.display_name(zh).to_owned(),
                configured: !self.youdao_app_key.is_empty() && !self.youdao_app_secret.is_empty(),
                mode: "v3".to_owned(),
                note: if !self.youdao_app_key.is_empty() && !self.youdao_app_secret.is_empty() {
                    if zh { "AppKey 与签名密钥已配置" } else { "AppKey & secret configured" }.to_owned()
                } else {
                    if zh { "需配置有道 AppKey 与 Secret" } else { "Configure AppKey & Secret in Settings" }.to_owned()
                },
            },
            EngineStatusEntry {
                id: "baidu",
                name: EngineId::Baidu.display_name(zh).to_owned(),
                configured: !self.baidu_app_id.is_empty() && !self.baidu_secret_key.is_empty(),
                mode: "vip".to_owned(),
                note: if !self.baidu_app_id.is_empty() && !self.baidu_secret_key.is_empty() {
                    if zh { "AppID 与密钥已配置" } else { "AppID & secret configured" }.to_owned()
                } else {
                    if zh { "需配置百度 AppID 与 Secret" } else { "Configure AppID & Secret in Settings" }.to_owned()
                },
            },
            EngineStatusEntry {
                id: "alibaba",
                name: EngineId::Alibaba.display_name(zh).to_owned(),
                configured: !self.alibaba_access_key_id.is_empty()
                    && !self.alibaba_access_key_secret.is_empty(),
                mode: self.alibaba_region_id.clone(),
                note: if !self.alibaba_access_key_id.is_empty()
                    && !self.alibaba_access_key_secret.is_empty()
                {
                    format!("{} · {}", self.alibaba_region_id, self.alibaba_scene)
                } else {
                    if zh { "需配置阿里云 AccessKey" } else { "Configure AccessKey in Settings" }.to_owned()
                },
            },
        ];

        ConfigSnapshot {
            effective_locale: self.effective_locale.clone(),
            is_zh: zh,
            engine: self.engine.as_str().to_owned(),
            instant_translation: self.instant_translation,
            primary_language: self.primary_language.clone(),
            secondary_language: self.secondary_language.clone(),
            source_language: self.source_language.clone(),
            target_language: self.target_language.clone(),
            target_language_selection: self.target_language_selection.clone(),
            auto_selection_mode: self.auto_selection_mode.clone(),
            auto_split_words: self.auto_split_words,
            separators: self.separators.clone(),
            ignore_regexp: self.ignore_regexp.clone(),
            show_hover_translation: self.show_hover_translation,
            show_doc_annotations: self.show_doc_annotations,
            preferred_naming_style: self.preferred_naming_style.clone(),
            cache_enabled: self.cache_enabled,
            cache_ttl_hours: self.cache_ttl_hours,
            max_history_items: self.max_history_items,
            show_words_on_startup: self.show_words_on_startup,
            keep_format: self.keep_format,
            strip_articles_in_identifier: self.strip_articles_in_identifier,
            auto_replace: self.auto_replace,
            select_target_language_before_replacement: self.select_target_language_before_replacement,
            doc_bilingual_mode: self.doc_bilingual_mode,
            fold_original_in_doc: self.fold_original_in_doc,
            open_doc_preview_tab: self.open_doc_preview_tab,
            preserve_inline_tags_in_doc: self.preserve_inline_tags_in_doc,
            tts_engine: self.tts_engine.as_str().to_owned(),
            tts_accent: self.tts_accent.clone(),
            edge_voice: self.edge_voice.clone(),
            azure_speech_region: self.azure_speech_region.clone(),
            tts_speed: self.tts_speed,
            tts_auto_play: self.tts_auto_play,
            tts_secrets_status: TtsSecretStatus {
                openai_tts_ready: openai_tts_key || openai_key || openai_tts_local,
                azure_speech_ready: azure_speech_key,
                has_separate_openai_tts_key: openai_tts_key,
            },
            engines,
        }
    }
}

pub fn set_engine_override(engine: EngineId) -> Result<(), Error> {
    storage::set(OVERRIDE_ENGINE, engine.as_str())
}

pub fn set_language_overrides(source: Option<&str>, target: Option<&str>) -> Result<(), Error> {
    if let Some(src) = source {
        storage::set(OVERRIDE_SOURCE_LANG, &normalize_lang_pref(Some(src), true, "auto"))?;
    }
    if let Some(tgt) = target {
        let norm = normalize_lang_pref(Some(tgt), true, "auto");
        storage::set(OVERRIDE_TARGET_LANG, &norm)?;
        if norm != "auto" {
            let _ = storage::set(STATE_LAST_TARGET_LANG, &norm);
        }
    }
    Ok(())
}

pub fn record_last_target_language(target: &str) {
    let norm = normalize_lang_pref(Some(target), true, "auto");
    if norm != "auto" {
        let _ = storage::set(STATE_LAST_TARGET_LANG, &norm);
    }
}

pub fn last_target_language() -> Option<String> {
    storage_string(STATE_LAST_TARGET_LANG)
        .map(|s| normalize_lang_pref(Some(&s), false, ""))
        .filter(|s| !s.is_empty() && s != "auto")
}

pub fn set_auto_split_override(enabled: bool) -> Result<(), Error> {
    storage::set(OVERRIDE_AUTO_SPLIT, if enabled { "true" } else { "false" })
}

pub fn set_instant_translation_override(enabled: bool) -> Result<(), Error> {
    storage::set(OVERRIDE_INSTANT, if enabled { "true" } else { "false" })
}

pub fn set_doc_bilingual_override(enabled: bool) -> Result<(), Error> {
    storage::set(OVERRIDE_DOC_BILINGUAL, if enabled { "true" } else { "false" })
}

pub fn set_tts_engine_override(tts: TtsEngineId) -> Result<(), Error> {
    storage::set(OVERRIDE_TTS_ENGINE, tts.as_str())
}

pub fn set_edge_voice_override(voice: &str) -> Result<(), Error> {
    let v = if voice.trim().is_empty() { "auto" } else { voice.trim() };
    storage::set(OVERRIDE_EDGE_VOICE, v)
}

pub fn clear_overrides_for_changed_keys(data: &str) {
    let keys: Vec<String> = serde_json::from_str(data).unwrap_or_default();
    let touches = |name: &str| {
        keys.is_empty()
            || keys
                .iter()
                .any(|k| k == name || k.ends_with(&format!(".{name}")))
    };
    if touches("engine") {
        let _ = storage::delete(OVERRIDE_ENGINE);
    }
    if touches("sourceLanguage") {
        let _ = storage::delete(OVERRIDE_SOURCE_LANG);
    }
    if touches("targetLanguage") {
        let _ = storage::delete(OVERRIDE_TARGET_LANG);
    }
    if touches("autoSplitWords") {
        let _ = storage::delete(OVERRIDE_AUTO_SPLIT);
    }
    if touches("showDocAnnotations") {
        let _ = storage::delete(OVERRIDE_INLINE_DOC);
    }
    if touches("instantTranslation") {
        let _ = storage::delete(OVERRIDE_INSTANT);
    }
    if touches("docBilingualMode") {
        let _ = storage::delete(OVERRIDE_DOC_BILINGUAL);
    }
    if touches("ttsEngine") {
        let _ = storage::delete(OVERRIDE_TTS_ENGINE);
    }
    if touches("edgeVoice") {
        let _ = storage::delete(OVERRIDE_EDGE_VOICE);
    }
}

pub fn is_loopback_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))
        .unwrap_or("");
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn host_display(url: &str) -> &str {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    rest.split(['/', '?', '#']).next().unwrap_or(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::mock::MockHost;

    #[test]
    fn snapshot_never_exposes_signing_secrets() {
        let host = MockHost::install().with_capabilities(&["storage", "secrets"]);
        host.set_setting("youdaoAppKey", "\"yd-key-123\"");
        host.set_setting("youdaoAppSecret", "\"SUPER_SECRET_YOUDAO\"");
        host.set_setting("baiduAppId", "\"bd-id-456\"");
        host.set_setting("baiduSecretKey", "\"SUPER_SECRET_BAIDU\"");
        host.set_setting("alibabaAccessKeyId", "\"ali-id-789\"");
        host.set_setting("alibabaAccessKeySecret", "\"SUPER_SECRET_ALIBABA\"");
        host.set_secret("azure-speech-api-key", "SECRET_AZURE_SPEECH");
        host.set_secret("openai-tts-api-key", "SECRET_OPENAI_TTS");

        let cfg = TranslationConfig::load();
        let snap = cfg.snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        assert!(!json.contains("SUPER_SECRET_YOUDAO"));
        assert!(!json.contains("SUPER_SECRET_BAIDU"));
        assert!(!json.contains("SUPER_SECRET_ALIBABA"));
        assert!(!json.contains("SECRET_AZURE_SPEECH"));
        assert!(!json.contains("SECRET_OPENAI_TTS"));
        assert!(snap.engines.iter().find(|e| e.id == "youdao").unwrap().configured);
        assert!(snap.engines.iter().find(|e| e.id == "baidu").unwrap().configured);
        assert!(snap.engines.iter().find(|e| e.id == "alibaba").unwrap().configured);
        assert!(snap.tts_secrets_status.azure_speech_ready);
        assert!(snap.tts_secrets_status.openai_tts_ready);
        assert!(snap.tts_secrets_status.has_separate_openai_tts_key);
    }

    #[test]
    fn settings_changed_clears_matching_overrides() {
        let host = MockHost::install().with_capabilities(&["storage", "events"]);
        set_engine_override(EngineId::Deepl).unwrap();
        set_language_overrides(Some("en"), Some("ja")).unwrap();
        set_tts_engine_override(TtsEngineId::Edge).unwrap();
        set_doc_bilingual_override(true).unwrap();
        assert_eq!(TranslationConfig::load().engine, EngineId::Deepl);
        assert_eq!(TranslationConfig::load().tts_engine, TtsEngineId::Edge);
        assert!(TranslationConfig::load().doc_bilingual_mode);

        host.set_setting("engine", "\"openai\"");
        clear_overrides_for_changed_keys(r#"["ruyimg.translation.engine"]"#);
        let cfg = TranslationConfig::load();
        assert_eq!(cfg.engine, EngineId::Openai);
        // Unchanged overrides stay intact
        assert_eq!(cfg.target_language, "ja");
        assert_eq!(cfg.tts_engine, TtsEngineId::Edge);
        assert!(cfg.doc_bilingual_mode);
    }
}
