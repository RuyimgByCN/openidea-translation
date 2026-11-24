pub mod config;
pub mod engines;
pub mod languages;
pub mod storage;
pub mod text_proc;
pub mod tts;

use crate::config::{ConfigSnapshot, EngineId, TranslationConfig, TtsEngineId};
use crate::engines::{EdgeToken, TranslationResult, translate_with_engine_mode, unix_millis_now};
use crate::languages::{LANGUAGES, display_name, resolve_lang_pair_with_strategy};
use crate::storage::{
    CacheEntry, HistoryItem, WordBookEntry, delete_wordbook_tag, export_wordbook, find_wordbook,
    import_wordbook_text, load_cache, load_history, load_wordbook, lookup_cache_by_term,
    lookup_cache_with_ttl, pick_word_of_the_day, prune_expired_cache, record_history,
    rename_wordbook_tag, save_json, save_wordbook, store_cache_entry_with_mode,
    upsert_wordbook_entry,
};
use crate::text_proc::{
    CommentBlock, ExtractedTarget, apply_ignore_regexp, apply_ipynb_translations,
    chunk_document_for_translation, extract_all_comments, extract_comment_block_at,
    extract_editor_target_with_options, extract_ipynb_markdown_cells,
    generate_replace_candidates_ext, preserve_format_wrapper, protect_inline_tags,
    restore_inline_tags, split_identifier_with_separators,
};
use crate::tts::{PendingEdgeTts, TtsSynthesis};
use openidea_plugin::hover::{Document as HoverDocument, Hover, HoverProvider};
use openidea_plugin::providers::{CodeAction, Document as ProviderDocument, Providers};
use openidea_plugin::search::{SearchItem, SearchPage, SearchProvider, SearchTarget};
use openidea_plugin::views::{View, ViewEvent, Viewport, Views};
use openidea_plugin::{
    ContentAction, ContentDiff, ContentTab, ContentTabAction, ContentTabReply, Context,
    Decoration, Error, Event, EventKind, Invocation, LogLevel, NotificationAction,
    NotificationLevel, PickItem, Plugin, Position, Range, StatusItem, StatusMenuEntry, TextEdit,
    TextEditor, host, register_plugin,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub const VIEW: &str = "translation";
pub const STATUS_ID: &str = "translation";
pub const DECORATION_SET_COMMENTS: &str = "translation-comments";
const MAX_TRANSLATE_BYTES: usize = 32_768;

#[derive(Clone, Debug)]
struct TargetContext {
    path: String,
    range: Range,
    version: u64,
}

#[derive(Clone, Debug)]
enum PendingAction {
    Translate {
        raw_text: String,
        query_text: String,
        source_pref: String,
        target_pref: String,
        engine: EngineId,
    },
    Tts {
        text: String,
        lang: Option<String>,
        accent: Option<String>,
    },
    TestConnection {
        engine: EngineId,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranslatedCommentItem {
    start_line: u32,
    end_line: u32,
    original: String,
    translation: String,
}

#[derive(Default)]
pub struct TranslationPlugin {
    cache: Vec<CacheEntry>,
    cache_loaded: bool,
    edge_token: Option<EdgeToken>,
    pending_request: Option<PendingAction>,
    pending_tts: HashMap<u64, PendingEdgeTts>,
    annotated_files: HashSet<String>,
    content_tab_sources: HashMap<String, (String, String)>,
    last_result: Option<TranslationResult>,
    last_target: Option<TargetContext>,
    startup_wotd_shown: bool,
}

impl TranslationPlugin {
    fn ensure_cache(&mut self, ttl_hours: u64) {
        if !self.cache_loaded {
            self.cache = load_cache();
            if prune_expired_cache(&mut self.cache, ttl_hours) {
                let _ = save_json(storage::KEY_CACHE, &self.cache);
            }
            self.cache_loaded = true;
        }
    }

    fn translate_cached_or_live(
        &mut self,
        raw_text: &str,
        query_text: &str,
        source_pref: &str,
        target_pref: &str,
        engine: EngineId,
        cfg: &TranslationConfig,
    ) -> Result<TranslationResult, Error> {
        self.translate_cached_or_live_ext(
            raw_text,
            query_text,
            source_pref,
            target_pref,
            engine,
            cfg,
            false,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn translate_cached_or_live_ext(
        &mut self,
        raw_text: &str,
        query_text: &str,
        source_pref: &str,
        target_pref: &str,
        engine: EngineId,
        cfg: &TranslationConfig,
        is_document: bool,
        record_to_history: bool,
    ) -> Result<TranslationResult, Error> {
        self.ensure_cache(cfg.cache_ttl_hours);
        if query_text.trim().is_empty() {
            return Err(Error::Invalid("Nothing to translate".into()));
        }
        if query_text.len() > MAX_TRANSLATE_BYTES {
            return Err(Error::Invalid(format!(
                "Selection exceeds {} KiB limit",
                MAX_TRANSLATE_BYTES / 1024
            )));
        }

        let last_tgt = config::last_target_language();
        let (resolved_src, resolved_tgt) = resolve_lang_pair_with_strategy(
            query_text,
            source_pref,
            target_pref,
            &cfg.primary_language,
            &cfg.secondary_language,
            &cfg.target_language_selection,
            last_tgt.as_deref(),
        );
        let mode_key = if is_document { "doc" } else { "text" };

        if cfg.cache_enabled
            && let Some(mut hit) = lookup_cache_with_ttl(
                &mut self.cache,
                engine.as_str(),
                &resolved_src,
                &resolved_tgt,
                mode_key,
                query_text,
                cfg.cache_ttl_hours,
            )
        {
            hit.original_text = raw_text.to_owned();
            if record_to_history {
                self.last_result = Some(hit.clone());
            }
            return Ok(hit);
        }

        let res = translate_with_engine_mode(
            engine,
            raw_text,
            query_text,
            &resolved_src,
            &resolved_tgt,
            cfg,
            &mut self.edge_token,
            is_document,
        )?;
        if cfg.cache_enabled {
            store_cache_entry_with_mode(
                &mut self.cache,
                engine.as_str(),
                &resolved_src,
                &resolved_tgt,
                mode_key,
                &res,
            );
        }
        if record_to_history {
            let _ = record_history(&res, cfg.max_history_items);
            self.last_result = Some(res.clone());
        }
        Ok(res)
    }

    fn post_result_to_view(&self, res: &TranslationResult, cfg: &TranslationConfig) {
        let candidates = generate_replace_candidates_ext(
            &res.translation,
            &res.alternatives,
            &cfg.preferred_naming_style,
            cfg.strip_articles_in_identifier,
            &cfg.separators,
        );
        let wordbook = load_wordbook();
        let in_wordbook =
            find_wordbook(&wordbook, &res.original_text, Some(&res.query_text)).is_some();
        let history = load_history();
        let payload = json!({
            "op": "translation-result",
            "result": res,
            "candidates": candidates,
            "inWordbook": in_wordbook,
            "history": history,
            "cacheCount": self.cache.len(),
        });
        let _ = host::post_view_message(VIEW, &payload.to_string());
    }

    fn post_full_state(&mut self, cfg: &TranslationConfig) {
        self.ensure_cache(cfg.cache_ttl_hours);
        let wordbook = load_wordbook();
        let history = load_history();
        let snap: ConfigSnapshot = cfg.snapshot();
        let wotd = pick_word_of_the_day(&wordbook).cloned();
        let payload = json!({
            "op": "init-state",
            "config": snap,
            "languages": LANGUAGES,
            "wordbook": wordbook,
            "history": history,
            "cacheCount": self.cache.len(),
            "lastResult": self.last_result,
            "wordOfTheDay": wotd,
        });
        let _ = host::post_view_message(VIEW, &payload.to_string());
    }

    fn post_config_only(&self, cfg: &TranslationConfig) {
        let payload = json!({
            "op": "config",
            "config": cfg.snapshot(),
            "cacheCount": self.cache.len(),
        });
        let _ = host::post_view_message(VIEW, &payload.to_string());
    }

    fn post_error_to_view(&self, op: &str, err: &Error) {
        let payload = json!({
            "op": "error",
            "sourceOp": op,
            "message": err.to_string(),
        });
        let _ = host::post_view_message(VIEW, &payload.to_string());
    }

    fn dispatch_tts(
        &mut self,
        text: &str,
        lang: Option<&str>,
        accent: Option<&str>,
        cfg: &TranslationConfig,
    ) -> Result<(), Error> {
        match tts::synthesize_with_ws(text, lang, accent, cfg)? {
            TtsSynthesis::Ready(audio) => {
                let _ = host::post_view_message(
                    VIEW,
                    &json!({ "op": "tts-audio", "audio": audio }).to_string(),
                );
            }
            TtsSynthesis::PendingEdge(pending) => {
                self.pending_tts.insert(pending.ws.id(), pending);
            }
        }
        Ok(())
    }

    fn notify_permission_retry(
        &mut self,
        action: PendingAction,
        capability: &str,
        cfg: &TranslationConfig,
    ) {
        self.pending_request = Some(action);
        let zh = cfg.is_zh();
        let title = if zh { "需要网络访问权限" } else { "Network Permission Needed" };
        let body = if zh {
            format!("访问 {capability} 需要您授权。点击下方按钮触发授权并重试。")
        } else {
            format!("Access to {capability} requires approval. Click below to authorize and retry.")
        };
        let label = if zh { "授权并重试" } else { "Allow & Retry" };
        let _ = host::notify_with_actions(
            NotificationLevel::Warning,
            title,
            &body,
            &[NotificationAction::new(label, "retry-pending-request")],
        );
    }

    fn extract_with_mode(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
        mode_override: Option<&str>,
    ) -> Result<(TextEditor, openidea_plugin::Document, ExtractedTarget), Error> {
        let editor = ctx
            .editor()
            .ok_or_else(|| Error::Invalid("No text editor is focused".into()))?;
        let doc = editor.document()?;
        let selections = editor.selections()?;
        let sel = selections.first().copied().unwrap_or_default();
        let mode = mode_override.unwrap_or(&cfg.auto_selection_mode);
        let target = extract_editor_target_with_options(
            &doc.text,
            sel,
            mode,
            cfg.auto_split_words,
            &cfg.separators,
            &cfg.ignore_regexp,
        )
        .ok_or_else(|| Error::Invalid("No word or text found at cursor".into()))?;
        self.last_target = Some(TargetContext {
            path: editor.path(),
            range: target.range,
            version: doc.version,
        });
        Ok((editor, doc, target))
    }

    fn extract_from_active_editor(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
    ) -> Result<(TextEditor, openidea_plugin::Document, ExtractedTarget), Error> {
        self.extract_with_mode(ctx, cfg, None)
    }

    fn handle_translate_command_with_mode(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
        mode_override: Option<&str>,
        engine_override: Option<EngineId>,
    ) -> Result<Option<String>, Error> {
        let (_editor, _doc, target) = self.extract_with_mode(ctx, cfg, mode_override)?;
        let engine = engine_override.unwrap_or(cfg.engine);
        let res = self.translate_cached_or_live(
            &target.raw_text,
            &target.query_text,
            &cfg.source_language,
            &cfg.target_language,
            engine,
            cfg,
        )?;
        let _ = host::reveal_view(VIEW);
        self.post_result_to_view(&res, cfg);
        if cfg.tts_auto_play && target.query_text.chars().count() <= 80 {
            let _ = self.dispatch_tts(&target.query_text, Some(&res.source_lang), None, cfg);
        }
        Ok(None)
    }

    fn handle_translate_and_copy(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
    ) -> Result<Option<String>, Error> {
        let (_editor, _doc, target) = self.extract_from_active_editor(ctx, cfg)?;
        let res = self.translate_cached_or_live(
            &target.raw_text,
            &target.query_text,
            &cfg.source_language,
            &cfg.target_language,
            cfg.engine,
            cfg,
        )?;
        host::copy_to_clipboard(&res.translation)?;
        self.post_result_to_view(&res, cfg);
        let clipped = truncate_label(&res.translation, 48);
        Ok(Some(if cfg.is_zh() {
            format!("已翻译并复制：{clipped}")
        } else {
            format!("Translated & copied: {clipped}")
        }))
    }

    fn handle_translate_with_engine(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
        arg: Option<&str>,
    ) -> Result<Option<String>, Error> {
        let chosen = if let Some(eng) = arg.and_then(EngineId::parse) {
            eng
        } else {
            let zh = cfg.is_zh();
            let items: Vec<PickItem> = EngineId::ALL
                .iter()
                .map(|e| {
                    let mark = if *e == cfg.engine {
                        if zh { "默认" } else { "Default" }
                    } else {
                        ""
                    };
                    let mut item = PickItem::new(e.as_str(), e.display_name(zh));
                    if !mark.is_empty() {
                        item = item.description(mark);
                    }
                    item
                })
                .collect();
            let title = if zh {
                "使用指定引擎翻译选区："
            } else {
                "Translate Selection with Engine:"
            };
            let Some(picked) = host::pick(title, &items)? else {
                return Ok(None);
            };
            EngineId::parse(&picked).ok_or_else(|| Error::Invalid("Invalid engine".into()))?
        };
        self.handle_translate_command_with_mode(ctx, cfg, None, Some(chosen))
    }

    fn handle_translate_clipboard(&mut self, cfg: &TranslationConfig) -> Result<Option<String>, Error> {
        let clip = host::read_clipboard()?;
        let raw_clip = clip.text.unwrap_or_default();
        let trimmed = raw_clip.trim();
        if trimmed.is_empty() {
            return Err(Error::Invalid(if cfg.is_zh() {
                "剪贴板中没有可翻译文本".into()
            } else {
                "Clipboard contains no text to translate".into()
            }));
        }
        let cleaned = apply_ignore_regexp(trimmed, &cfg.ignore_regexp);
        if cleaned.trim().is_empty() {
            return Err(Error::Invalid("Clipboard text stripped by ignoreRegExp".into()));
        }
        let query = if cfg.auto_split_words {
            split_identifier_with_separators(&cleaned, &cfg.separators)
                .unwrap_or_else(|| cleaned.clone())
        } else {
            cleaned
        };
        let res = self.translate_cached_or_live(
            trimmed,
            &query,
            &cfg.source_language,
            &cfg.target_language,
            cfg.engine,
            cfg,
        )?;
        let _ = host::reveal_view(VIEW);
        self.post_result_to_view(&res, cfg);
        Ok(None)
    }

    fn handle_word_of_the_day(&self, cfg: &TranslationConfig) -> Result<Option<String>, Error> {
        let zh = cfg.is_zh();
        let wordbook = load_wordbook();
        let Some(entry) = pick_word_of_the_day(&wordbook) else {
            return Ok(Some(if zh {
                "生词本暂无词汇，先收藏一些单词吧。".into()
            } else {
                "Word Book is empty — add words to see Word of the Day.".into()
            }));
        };
        let phonetic_part = if entry.phonetic.is_empty() {
            String::new()
        } else {
            format!(" /{}/", entry.phonetic.trim_matches('/'))
        };
        let title = if zh {
            format!("每日一词：{}{} ★", entry.word, phonetic_part)
        } else {
            format!("Word of the Day: {}{}", entry.word, phonetic_part)
        };
        let body = if entry.explains.is_empty() {
            entry.translation.clone()
        } else {
            format!("{} — {}", entry.translation, entry.explains[0])
        };
        let _ = host::notify_with_actions(
            NotificationLevel::Info,
            &title,
            &body,
            &[NotificationAction::new(
                if zh { "在生词本中查看" } else { "Open Word Book" },
                "open-wordbook",
            )
            .argument(&entry.word)],
        );
        let _ = host::post_view_message(
            VIEW,
            &json!({ "op": "word-of-the-day", "entry": entry }).to_string(),
        );
        Ok(None)
    }

    fn handle_replace_command(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
    ) -> Result<Option<String>, Error> {
        let editor = ctx
            .editor()
            .ok_or_else(|| Error::Invalid("No text editor is focused".into()))?;
        let doc = editor.document()?;
        let selections = editor.selections()?;
        if selections.is_empty() {
            return Err(Error::Invalid("No cursor or selection found".into()));
        }

        // Optional prompt to select target language before replacing
        let target_override = if cfg.select_target_language_before_replacement {
            let zh = cfg.is_zh();
            let items: Vec<PickItem> = LANGUAGES
                .iter()
                .filter(|l| l.code != "auto")
                .map(|l| {
                    PickItem::new(l.code, if zh { l.zh_name } else { l.en_name }).description(l.code)
                })
                .collect();
            let prompt = if zh {
                "替换前选择目标语言："
            } else {
                "Select Target Language for Replacement:"
            };
            match host::pick(prompt, &items)? {
                Some(code) => code,
                None => return Ok(None),
            }
        } else {
            cfg.target_language.clone()
        };

        let mut targets = Vec::new();
        if selections.len() == 1 {
            let sel = selections[0];
            let target = extract_editor_target_with_options(
                &doc.text,
                sel,
                &cfg.auto_selection_mode,
                cfg.auto_split_words,
                &cfg.separators,
                &cfg.ignore_regexp,
            )
            .ok_or_else(|| Error::Invalid("No word or text found at cursor".into()))?;
            targets.push(target);
        } else {
            for sel in &selections {
                if let Some(target) = extract_editor_target_with_options(
                    &doc.text,
                    *sel,
                    &cfg.auto_selection_mode,
                    cfg.auto_split_words,
                    &cfg.separators,
                    &cfg.ignore_regexp,
                ) {
                    targets.push(target);
                }
            }
            if targets.is_empty() {
                return Err(Error::Invalid("No words found at multi-cursor positions".into()));
            }
        }

        self.last_target = Some(TargetContext {
            path: editor.path(),
            range: targets[0].range,
            version: doc.version,
        });

        let mut edits = Vec::with_capacity(targets.len());
        let is_single = targets.len() == 1;

        for target in &targets {
            let slice_raw = doc
                .text_in(target.range)
                .unwrap_or(&target.raw_text)
                .to_owned();
            let (prefix, core_raw, suffix) = if cfg.keep_format {
                let (p, c, s) = preserve_format_wrapper(&slice_raw);
                (p.to_owned(), c.to_owned(), s.to_owned())
            } else {
                (String::new(), target.raw_text.clone(), String::new())
            };
            let cleaned = apply_ignore_regexp(&core_raw, &cfg.ignore_regexp);
            let query = if cfg.auto_split_words {
                split_identifier_with_separators(&cleaned, &cfg.separators)
                    .unwrap_or_else(|| cleaned.clone())
            } else {
                cleaned
            };

            let res = self.translate_cached_or_live(
                &core_raw,
                &query,
                &cfg.source_language,
                &target_override,
                cfg.engine,
                cfg,
            )?;
            if is_single {
                self.post_result_to_view(&res, cfg);
            }

            let candidates = generate_replace_candidates_ext(
                &res.translation,
                &res.alternatives,
                &cfg.preferred_naming_style,
                cfg.strip_articles_in_identifier,
                &cfg.separators,
            );
            if candidates.is_empty() {
                return Err(Error::Failed("Translation produced empty replacement".into()));
            }

            let chosen = if cfg.auto_replace || !is_single || candidates.len() == 1 {
                candidates[0].text.clone()
            } else {
                let items: Vec<PickItem> = candidates
                    .iter()
                    .map(|c| PickItem::new(&c.text, &c.text).description(&c.style))
                    .collect();
                let title = if cfg.is_zh() {
                    format!("替换 \"{}\" 为：", core_raw)
                } else {
                    format!("Replace \"{}\" with:", core_raw)
                };
                match host::pick(&title, &items)? {
                    Some(choice) => choice,
                    None => return Ok(None),
                }
            };

            edits.push(TextEdit {
                range: target.range,
                text: format!("{prefix}{chosen}{suffix}"),
            });
        }

        editor.apply_edits(doc.version, &edits)?;
        Ok(None)
    }

    fn handle_translate_doc_command(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
    ) -> Result<Option<String>, Error> {
        let editor = ctx
            .editor()
            .ok_or_else(|| Error::Invalid("No text editor is focused".into()))?;
        let doc = editor.document()?;
        let selections = editor.selections()?;
        let sel = selections.first().copied().unwrap_or_default();

        let (raw, clean, range) = if let Some(block) = extract_comment_block_at(&doc.text, sel.start) {
            (block.raw_text, block.clean_text, block.range)
        } else if !sel.is_empty() {
            let text = doc
                .text_in(sel)
                .ok_or_else(|| Error::Invalid("Invalid selection range".into()))?
                .to_owned();
            (text.clone(), text, sel)
        } else {
            return Err(Error::Invalid(
                "Place the cursor inside a comment/doc block or select documentation text".into(),
            ));
        };

        let cleaned = apply_ignore_regexp(&clean, &cfg.ignore_regexp);
        let protected = if cfg.preserve_inline_tags_in_doc {
            Some(protect_inline_tags(&cleaned))
        } else {
            None
        };
        let query = protected
            .as_ref()
            .map_or(cleaned.as_str(), |p| p.text.as_str());

        let mut res = self.translate_cached_or_live_ext(
            &raw,
            query,
            &cfg.source_language,
            &cfg.target_language,
            cfg.engine,
            cfg,
            true,
            true,
        )?;
        if let Some(prot) = &protected {
            res.translation = restore_inline_tags(&res.translation, &prot.tags);
            res.query_text = cleaned.clone();
        }

        self.last_target = Some(TargetContext {
            path: editor.path(),
            range,
            version: doc.version,
        });

        let _ = host::reveal_view(VIEW);
        self.post_result_to_view(&res, cfg);
        let _ = host::post_view_message(
            VIEW,
            &json!({
                "op": "doc-comment-result",
                "original": clean,
                "translation": res.translation,
                "sourceLang": res.source_lang,
                "targetLang": res.target_lang,
                "engine": res.engine,
            })
            .to_string(),
        );
        if cfg.open_doc_preview_tab && clean.lines().count() >= 4 {
            let content = if cfg.doc_bilingual_mode {
                format!("{clean}\n\n---\n\n{}", res.translation)
            } else {
                res.translation.clone()
            };
            let _ = host::open_content_tab(&ContentTab {
                key: format!("doc-comment:{}", editor.path()),
                title: if cfg.is_zh() { "文档注释译文".into() } else { "Doc Comment Translation".into() },
                file_name: Some("doc-comment.md".into()),
                language: Some("markdown".into()),
                content,
                read_only: true,
                actions: vec![ContentAction {
                    id: "copy".into(),
                    label: if cfg.is_zh() { "复制译文" } else { "Copy" }.into(),
                    icon: None,
                    primary: true,
                    save: false,
                }],
            });
        }
        Ok(None)
    }

    fn handle_toggle_doc_annotations(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
    ) -> Result<Option<String>, Error> {
        if !cfg.show_doc_annotations {
            return Err(Error::Invalid(
                "Inline comment annotations are disabled in settings".into(),
            ));
        }
        let editor = ctx
            .editor()
            .ok_or_else(|| Error::Invalid("No text editor is focused".into()))?;
        let path = editor.path();

        if self.annotated_files.remove(&path) {
            let _ = host::set_decorations(&path, DECORATION_SET_COMMENTS, &[]);
            let msg = if cfg.is_zh() {
                "已隐藏行内注释翻译"
            } else {
                "Inline comment translations cleared"
            };
            let _ = host::post_view_message(
                VIEW,
                &json!({ "op": "doc-annotations-status", "active": false, "path": path })
                    .to_string(),
            );
            return Ok(Some(msg.into()));
        }

        let doc = editor.document()?;
        let blocks = extract_all_comments(&doc.text, 60);
        if blocks.is_empty() {
            return Ok(Some(if cfg.is_zh() {
                "当前文件未检测到注释块".into()
            } else {
                "No comment blocks found in the active file".into()
            }));
        }

        let translated = self.translate_comment_blocks(&blocks, cfg)?;
        let decorations: Vec<Decoration> = translated
            .iter()
            .map(|item| {
                let single_line = item
                    .translation
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let clipped: String = single_line.chars().take(110).collect();
                let end_pos = blocks
                    .iter()
                    .find(|b| b.start_line == item.start_line)
                    .map_or_else(|| Position::new(item.end_line, 0), |b| b.range.end);
                Decoration {
                    range: Range::new(end_pos, end_pos),
                    style: "muted".into(),
                    hover: Some(format!(
                        "**Translation ({})**\n\n{}",
                        cfg.engine.short_label(),
                        item.translation
                    )),
                    after: Some(format!("  // {clipped}")),
                }
            })
            .collect();

        host::set_decorations(&path, DECORATION_SET_COMMENTS, &decorations)?;
        self.annotated_files.insert(path.clone());
        let _ = host::post_view_message(
            VIEW,
            &json!({
                "op": "doc-annotations-status",
                "active": true,
                "path": path,
                "items": translated,
            })
            .to_string(),
        );
        Ok(Some(if cfg.is_zh() {
            format!("已为 {} 条注释添加行内翻译", decorations.len())
        } else {
            format!("Annotated {} comment block(s)", decorations.len())
        }))
    }

    fn translate_comment_blocks(
        &mut self,
        blocks: &[CommentBlock],
        cfg: &TranslationConfig,
    ) -> Result<Vec<TranslatedCommentItem>, Error> {
        let task = host::begin_task(
            if cfg.is_zh() {
                "正在翻译文档注释..."
            } else {
                "Translating comments..."
            },
            true,
        );
        let total = blocks.len() as u32;
        let mut out = Vec::with_capacity(blocks.len());
        for (idx, block) in blocks.iter().enumerate() {
            if task.is_cancelled() {
                return Err(Error::Cancelled);
            }
            task.report(idx as u32, Some(total), None);
            let cleaned = apply_ignore_regexp(&block.clean_text, &cfg.ignore_regexp);
            let protected = if cfg.preserve_inline_tags_in_doc {
                Some(protect_inline_tags(&cleaned))
            } else {
                None
            };
            let query = protected
                .as_ref()
                .map_or(cleaned.as_str(), |p| p.text.as_str());
            let mut res = self.translate_cached_or_live_ext(
                &block.raw_text,
                query,
                &cfg.source_language,
                &cfg.target_language,
                cfg.engine,
                cfg,
                true,
                false,
            )?;
            if let Some(prot) = &protected {
                res.translation = restore_inline_tags(&res.translation, &prot.tags);
            }
            out.push(TranslatedCommentItem {
                start_line: block.start_line,
                end_line: block.end_line,
                original: block.clean_text.clone(),
                translation: res.translation,
            });
        }
        task.report(total, Some(total), None);
        Ok(out)
    }

    fn translate_chunk_text(
        &mut self,
        core: &str,
        cfg: &TranslationConfig,
    ) -> Result<TranslationResult, Error> {
        let cleaned = apply_ignore_regexp(core, &cfg.ignore_regexp);
        let query_base = if cleaned.trim().is_empty() {
            core.to_owned()
        } else {
            cleaned
        };
        let protected = if cfg.preserve_inline_tags_in_doc {
            Some(protect_inline_tags(&query_base))
        } else {
            None
        };
        let query = protected
            .as_ref()
            .map_or(query_base.as_str(), |p| p.text.as_str());
        let mut res = self.translate_cached_or_live_ext(
            core,
            query,
            &cfg.source_language,
            &cfg.target_language,
            cfg.engine,
            cfg,
            true,
            false,
        )?;
        if let Some(prot) = &protected {
            res.translation = restore_inline_tags(&res.translation, &prot.tags);
        }
        Ok(res)
    }

    fn handle_translate_document(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
    ) -> Result<Option<String>, Error> {
        let editor = ctx
            .editor()
            .ok_or_else(|| Error::Invalid("No text editor is focused".into()))?;
        let doc = editor.document()?;
        let path = editor.path();
        let lang = editor.language();
        if doc.text.trim().is_empty() {
            return Err(Error::Invalid("The active document is empty".into()));
        }

        let is_ipynb = path.to_ascii_lowercase().ends_with(".ipynb");
        let is_markdown = lang.eq_ignore_ascii_case("markdown")
            || path.to_ascii_lowercase().ends_with(".md")
            || path.to_ascii_lowercase().ends_with(".mdx");

        let mut resolved_src = cfg.source_language.clone();
        let mut resolved_tgt = cfg.target_language.clone();

        let translated_buf = if is_ipynb {
            let cells = extract_ipynb_markdown_cells(&doc.text).map_err(Error::Invalid)?;
            if cells.is_empty() {
                return Err(Error::Invalid(
                    "No Markdown or raw cells found in .ipynb notebook".into(),
                ));
            }
            let task = host::begin_task(
                if cfg.is_zh() {
                    "正在翻译 Jupyter Notebook 笔记本..."
                } else {
                    "Translating Jupyter Notebook..."
                },
                true,
            );
            let total = cells.len() as u32;
            let mut replacements = Vec::with_capacity(cells.len());
            for (idx, cell) in cells.iter().enumerate() {
                if task.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                task.report(idx as u32, Some(total), None);
                let cell_chunks = chunk_document_for_translation(&cell.source, "markdown", 1_600);
                let mut cell_out = String::new();
                for c in cell_chunks {
                    if !c.translatable || c.text.trim().is_empty() {
                        cell_out.push_str(&c.text);
                        continue;
                    }
                    let lws = c.text.len() - c.text.trim_start().len();
                    let tws = c.text.len() - c.text.trim_end().len();
                    let res = self.translate_chunk_text(c.text.trim(), cfg)?;
                    resolved_src = res.source_lang;
                    resolved_tgt = res.target_lang;
                    cell_out.push_str(&c.text[..lws]);
                    cell_out.push_str(&res.translation);
                    if tws > 0 {
                        cell_out.push_str(&c.text[c.text.len() - tws..]);
                    }
                }
                replacements.push((cell.cell_index, cell_out));
            }
            task.report(total, Some(total), None);
            apply_ipynb_translations(
                &doc.text,
                &replacements,
                cfg.doc_bilingual_mode,
                cfg.fold_original_in_doc,
            )
            .map_err(Error::Failed)?
        } else {
            let chunks = chunk_document_for_translation(&doc.text, &lang, 1_800);
            let translatable_count = chunks
                .iter()
                .filter(|c| c.translatable && !c.text.trim().is_empty())
                .count() as u32;
            let task = host::begin_task(
                if cfg.is_zh() {
                    "正在翻译完整文档..."
                } else {
                    "Translating document..."
                },
                true,
            );

            let mut out_buf = String::with_capacity(doc.text.len());
            let mut done = 0u32;

            for chunk in chunks {
                if task.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                if !chunk.translatable || chunk.text.trim().is_empty() {
                    out_buf.push_str(&chunk.text);
                    continue;
                }
                let leading_ws_len = chunk.text.len() - chunk.text.trim_start().len();
                let trailing_ws_len = chunk.text.len() - chunk.text.trim_end().len();
                let core = chunk.text.trim();
                let res = self.translate_chunk_text(core, cfg)?;
                resolved_src = res.source_lang;
                resolved_tgt = res.target_lang;
                out_buf.push_str(&chunk.text[..leading_ws_len]);
                if cfg.doc_bilingual_mode {
                    if is_markdown && cfg.fold_original_in_doc {
                        out_buf.push_str(&format!(
                            "<details>\n<summary>Original</summary>\n\n{core}\n\n</details>\n\n{}",
                            res.translation
                        ));
                    } else {
                        out_buf.push_str(&format!("{core}\n\n{}", res.translation));
                    }
                } else {
                    out_buf.push_str(&res.translation);
                }
                if trailing_ws_len > 0 {
                    out_buf.push_str(&chunk.text[chunk.text.len() - trailing_ws_len..]);
                }
                done += 1;
                task.report(done, Some(translatable_count.max(1)), None);
            }
            out_buf
        };

        let base_name = path
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("document.txt");
        let tab_key = format!("doc-trans:{base_name}");
        self.content_tab_sources
            .insert(tab_key.clone(), (doc.text.clone(), path.clone()));

        let zh = cfg.is_zh();
        if cfg.open_doc_preview_tab {
            let tab = ContentTab {
                key: tab_key,
                title: format!("{base_name} ({resolved_tgt})"),
                file_name: Some(format!("translated-{base_name}")),
                language: Some(lang),
                content: translated_buf.clone(),
                read_only: false,
                actions: vec![
                    ContentAction {
                        id: "diff".into(),
                        label: if zh { "对比原文" } else { "Compare Diff" }.into(),
                        icon: None,
                        primary: false,
                        save: false,
                    },
                    ContentAction {
                        id: "copy".into(),
                        label: if zh { "复制译文" } else { "Copy" }.into(),
                        icon: None,
                        primary: false,
                        save: false,
                    },
                    ContentAction {
                        id: "apply".into(),
                        label: if zh { "应用到原文件" } else { "Apply to Editor" }.into(),
                        icon: None,
                        primary: true,
                        save: true,
                    },
                ],
            };
            let _ = host::open_content_tab(&tab);
        }

        let _ = host::post_view_message(
            VIEW,
            &json!({
                "op": "doc-file-result",
                "fileName": base_name,
                "sourceLang": resolved_src,
                "targetLang": resolved_tgt,
                "bilingual": cfg.doc_bilingual_mode,
                "isIpynb": is_ipynb,
                "original": doc.text,
                "translation": translated_buf,
            })
            .to_string(),
        );
        Ok(if !cfg.open_doc_preview_tab {
            Some(if zh {
                format!("已完成文档翻译：{base_name}")
            } else {
                format!("Translated document: {base_name}")
            })
        } else {
            None
        })
    }

    fn handle_quick_translate(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
    ) -> Result<Option<String>, Error> {
        let prefill = ctx.editor().and_then(|e| {
            let doc = e.document().ok()?;
            let sel = e.selections().ok()?.first().copied()?;
            extract_editor_target_with_options(
                &doc.text,
                sel,
                &cfg.auto_selection_mode,
                false,
                &cfg.separators,
                &cfg.ignore_regexp,
            )
            .map(|t| t.raw_text)
        });
        let prompt = if cfg.is_zh() {
            "输入要翻译的文本或代码标识符："
        } else {
            "Enter text or identifier to translate:"
        };
        let Some(input) = host::input(prompt, prefill.as_deref())? else {
            return Ok(None);
        };
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let cleaned = apply_ignore_regexp(trimmed, &cfg.ignore_regexp);
        let query = if cfg.auto_split_words {
            split_identifier_with_separators(&cleaned, &cfg.separators)
                .unwrap_or_else(|| cleaned.clone())
        } else {
            cleaned
        };
        let res = self.translate_cached_or_live(
            trimmed,
            &query,
            &cfg.source_language,
            &cfg.target_language,
            cfg.engine,
            cfg,
        )?;
        let _ = host::reveal_view(VIEW);
        self.post_result_to_view(&res, cfg);
        Ok(None)
    }

    fn handle_speak_selection(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
    ) -> Result<Option<String>, Error> {
        let (text, lang_hint) = if let Ok((_, _, target)) = self.extract_from_active_editor(ctx, cfg)
        {
            (target.query_text, None)
        } else if let Some(last) = &self.last_result {
            (last.query_text.clone(), Some(last.source_lang.clone()))
        } else {
            return Err(Error::Invalid("Select text or translate a word first".into()));
        };
        let _ = host::reveal_view(VIEW);
        self.dispatch_tts(&text, lang_hint.as_deref(), None, cfg)?;
        Ok(None)
    }

    fn handle_add_to_wordbook(
        &mut self,
        ctx: &Invocation,
        cfg: &TranslationConfig,
    ) -> Result<Option<String>, Error> {
        let (editor, _doc, target) = self.extract_from_active_editor(ctx, cfg)?;
        let path = editor.path();
        let res = self.translate_cached_or_live(
            &target.raw_text,
            &target.query_text,
            &cfg.source_language,
            &cfg.target_language,
            cfg.engine,
            cfg,
        )?;
        let phonetic = res
            .phonetics
            .us
            .clone()
            .or_else(|| res.phonetics.uk.clone())
            .or_else(|| res.phonetics.general.clone())
            .unwrap_or_default();
        let explains: Vec<String> = res
            .dict
            .iter()
            .flat_map(|d| {
                if !d.explains.is_empty() {
                    d.explains.clone()
                } else if d.pos.is_empty() {
                    vec![d.terms.join(", ")]
                } else {
                    vec![format!("{} {}", d.pos, d.terms.join(", "))]
                }
            })
            .take(6)
            .collect();

        let entry = WordBookEntry {
            id: String::new(),
            word: target.raw_text.clone(),
            source_lang: res.source_lang.clone(),
            target_lang: res.target_lang.clone(),
            translation: res.translation.clone(),
            phonetic,
            explains,
            tags: vec![editor.language()],
            note: String::new(),
            source_file: path
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&path)
                .to_owned(),
            created_at_ms: unix_millis_now(),
            review_count: 0,
            mastered: false,
        };
        let (list, _, created) = upsert_wordbook_entry(entry)?;
        self.post_result_to_view(&res, cfg);
        let _ = host::post_view_message(
            VIEW,
            &json!({ "op": "wordbook-sync", "wordbook": list }).to_string(),
        );
        Ok(Some(if cfg.is_zh() {
            if created {
                format!("已将 \"{}\" 添加到生词本", target.raw_text)
            } else {
                format!("已更新生词本中的 \"{}\"", target.raw_text)
            }
        } else if created {
            format!("Added \"{}\" to Word Book", target.raw_text)
        } else {
            format!("Updated \"{}\" in Word Book", target.raw_text)
        }))
    }
}

pub fn refresh_status_menu(cfg: &TranslationConfig) {
    let zh = cfg.is_zh();
    let item = StatusItem {
        text: format!("{} · {}", if zh { "翻译" } else { "Translate" }, cfg.engine.short_label()),
        tooltip: Some(if zh {
            format!(
                "Translation — 引擎: {} | 目标: {}",
                cfg.engine.display_name(true),
                cfg.target_language
            )
        } else {
            format!(
                "Translation — Engine: {} | Target: {}",
                cfg.engine.display_name(false),
                cfg.target_language
            )
        }),
        command: Some("open-panel".into()),
    };

    let mut menu = Vec::new();
    menu.push(StatusMenuEntry::action(
        if zh { "打开翻译面板" } else { "Open Translation Panel" },
        "open-panel",
    ));
    menu.push(StatusMenuEntry::action(
        if zh { "打开生词本" } else { "Open Word Book" },
        "open-wordbook",
    ));
    menu.push(StatusMenuEntry::action(
        if zh { "每日一词" } else { "Word of the Day" },
        "word-of-the-day",
    ));
    menu.push(StatusMenuEntry::action(
        if zh { "快捷输入翻译..." } else { "Quick Translate Input..." },
        "quick-translate",
    ));
    menu.push(StatusMenuEntry::action(
        if zh { "翻译剪贴板" } else { "Translate Clipboard" },
        "translate-clipboard",
    ));
    menu.push(StatusMenuEntry::Separator);
    menu.push(StatusMenuEntry::Header(
        if zh { "切换翻译引擎" } else { "Translation Engine" }.into(),
    ));
    for engine in EngineId::ALL {
        let prefix = if engine == cfg.engine { "✓ " } else { "" };
        menu.push(
            StatusMenuEntry::action(format!("{prefix}{}", engine.display_name(zh)), "switch-engine")
                .with_argument(engine.as_str()),
        );
    }
    menu.push(StatusMenuEntry::Separator);
    menu.push(StatusMenuEntry::action(
        if zh { "互换源/目标语言" } else { "Swap Languages" },
        "swap-languages",
    ));
    menu.push(StatusMenuEntry::action(
        if zh { "测试当前引擎连接" } else { "Test Connection" },
        "test-connection",
    ));
    menu.push(StatusMenuEntry::action(
        if zh { "清空翻译缓存" } else { "Clear Cache" },
        "clear-cache",
    ));

    let _ = host::set_status_item_menu(STATUS_ID, &item, &menu);
}

impl Plugin for TranslationPlugin {
    fn activate(&mut self, _ctx: &Context) -> Result<(), Error> {
        let cfg = TranslationConfig::load();
        self.ensure_cache(cfg.cache_ttl_hours);
        let _ = host::subscribe(EventKind::SettingsChanged);
        let _ = host::subscribe(EventKind::ContentTabAction);
        let _ = host::subscribe(EventKind::WebsocketReady);
        refresh_status_menu(&cfg);
        if !self.startup_wotd_shown && cfg.show_words_on_startup {
            self.startup_wotd_shown = true;
            let _ = self.handle_word_of_the_day(&cfg);
        }
        Ok(())
    }

    fn run_command(
        &mut self,
        id: &str,
        ctx: &Invocation,
        arg: Option<&str>,
    ) -> Result<Option<String>, Error> {
        let cfg = TranslationConfig::load();
        match id {
            "translate" => self.handle_translate_command_with_mode(ctx, &cfg, None, None),
            "translate-inclusive" => {
                self.handle_translate_command_with_mode(ctx, &cfg, Some("inclusive"), None)
            }
            "translate-exclusive" => {
                self.handle_translate_command_with_mode(ctx, &cfg, Some("exclusive"), None)
            }
            "translate-and-replace" => self.handle_replace_command(ctx, &cfg),
            "translate-and-copy" => self.handle_translate_and_copy(ctx, &cfg),
            "translate-with-engine" => self.handle_translate_with_engine(ctx, &cfg, arg),
            "translate-doc" => self.handle_translate_doc_command(ctx, &cfg),
            "toggle-doc-annotations" => self.handle_toggle_doc_annotations(ctx, &cfg),
            "translate-document" => self.handle_translate_document(ctx, &cfg),
            "quick-translate" => self.handle_quick_translate(ctx, &cfg),
            "translate-clipboard" => self.handle_translate_clipboard(&cfg),
            "speak-selection" => self.handle_speak_selection(ctx, &cfg),
            "add-to-wordbook" => self.handle_add_to_wordbook(ctx, &cfg),
            "word-of-the-day" => self.handle_word_of_the_day(&cfg),
            "open-panel" => {
                host::reveal_view(VIEW)?;
                self.post_full_state(&cfg);
                Ok(None)
            }
            "open-wordbook" => {
                host::reveal_view(VIEW)?;
                self.post_full_state(&cfg);
                let _ = host::post_view_message(
                    VIEW,
                    &json!({ "op": "navigate", "tab": "wordbook", "query": arg.unwrap_or("") })
                        .to_string(),
                );
                Ok(None)
            }
            "switch-engine" => {
                let chosen = if let Some(raw) = arg.and_then(EngineId::parse) {
                    raw
                } else {
                    let zh = cfg.is_zh();
                    let items: Vec<PickItem> = EngineId::ALL
                        .iter()
                        .map(|e| {
                            let mark = if *e == cfg.engine {
                                if zh { "当前" } else { "Active" }
                            } else {
                                ""
                            };
                            let mut item = PickItem::new(e.as_str(), e.display_name(zh));
                            if !mark.is_empty() {
                                item = item.description(mark);
                            }
                            item
                        })
                        .collect();
                    let title = if zh { "选择翻译引擎" } else { "Select Translation Engine" };
                    let Some(picked) = host::pick(title, &items)? else {
                        return Ok(None);
                    };
                    EngineId::parse(&picked).ok_or_else(|| Error::Invalid("Invalid engine".into()))?
                };
                config::set_engine_override(chosen)?;
                let updated = TranslationConfig::load();
                refresh_status_menu(&updated);
                self.post_config_only(&updated);
                Ok(Some(if updated.is_zh() {
                    format!("已切换翻译引擎：{}", chosen.display_name(true))
                } else {
                    format!("Translation engine switched to {}", chosen.display_name(false))
                }))
            }
            "switch-target-language" => {
                let zh = cfg.is_zh();
                let chosen = if let Some(code) = arg.filter(|s| !s.trim().is_empty()) {
                    code.trim().to_owned()
                } else {
                    let items: Vec<PickItem> = LANGUAGES
                        .iter()
                        .map(|l| {
                            PickItem::new(l.code, if zh { l.zh_name } else { l.en_name })
                                .description(l.code)
                        })
                        .collect();
                    let title = if zh { "选择目标语言" } else { "Select Target Language" };
                    let Some(picked) = host::pick(title, &items)? else {
                        return Ok(None);
                    };
                    picked
                };
                config::set_language_overrides(None, Some(&chosen))?;
                let updated = TranslationConfig::load();
                refresh_status_menu(&updated);
                self.post_config_only(&updated);
                Ok(Some(if zh {
                    format!("目标语言已设为：{}", display_name(&updated.target_language, true))
                } else {
                    format!(
                        "Target language set to {}",
                        display_name(&updated.target_language, false)
                    )
                }))
            }
            "swap-languages" => {
                let (next_src, next_tgt) =
                    if cfg.source_language == "auto" && cfg.target_language == "auto" {
                        if let Some(last) = &self.last_result {
                            (last.target_lang.clone(), last.source_lang.clone())
                        } else {
                            (cfg.primary_language.clone(), cfg.secondary_language.clone())
                        }
                    } else if cfg.source_language == "auto" {
                        let fallback = if cfg.target_language == cfg.primary_language {
                            cfg.secondary_language.clone()
                        } else {
                            cfg.primary_language.clone()
                        };
                        (cfg.target_language.clone(), fallback)
                    } else {
                        (cfg.target_language.clone(), cfg.source_language.clone())
                    };
                config::set_language_overrides(Some(&next_src), Some(&next_tgt))?;
                let updated = TranslationConfig::load();
                refresh_status_menu(&updated);
                self.post_config_only(&updated);
                Ok(None)
            }
            "clear-cache" => {
                self.ensure_cache(cfg.cache_ttl_hours);
                storage::clear_cache(&mut self.cache)?;
                self.post_config_only(&cfg);
                Ok(Some(if cfg.is_zh() {
                    "翻译缓存已清空".into()
                } else {
                    "Translation cache cleared".into()
                }))
            }
            "test-connection" => {
                let sample_src = if cfg.secondary_language.is_empty() {
                    "en"
                } else {
                    &cfg.secondary_language
                };
                let sample_tgt = if cfg.primary_language == sample_src {
                    "en"
                } else {
                    &cfg.primary_language
                };
                let res = translate_with_engine_mode(
                    cfg.engine,
                    "Hello",
                    "Hello",
                    sample_src,
                    sample_tgt,
                    &cfg,
                    &mut self.edge_token,
                    false,
                )?;
                let msg = if cfg.is_zh() {
                    format!(
                        "{} 连接成功 ({}ms): Hello → {}",
                        cfg.engine.display_name(true),
                        res.latency_ms,
                        res.translation
                    )
                } else {
                    format!(
                        "{} OK ({}ms): Hello -> {}",
                        cfg.engine.display_name(false),
                        res.latency_ms,
                        res.translation
                    )
                };
                let _ = host::post_view_message(
                    VIEW,
                    &json!({ "op": "test-connection-result", "ok": true, "message": msg })
                        .to_string(),
                );
                Ok(Some(msg))
            }
            "retry-pending-request" => {
                let Some(pending) = self.pending_request.take() else {
                    return Ok(None);
                };
                match pending {
                    PendingAction::Translate {
                        raw_text,
                        query_text,
                        source_pref,
                        target_pref,
                        engine,
                    } => {
                        let res = self.translate_cached_or_live(
                            &raw_text,
                            &query_text,
                            &source_pref,
                            &target_pref,
                            engine,
                            &cfg,
                        )?;
                        let _ = host::reveal_view(VIEW);
                        self.post_result_to_view(&res, &cfg);
                        Ok(None)
                    }
                    PendingAction::Tts { text, lang, accent } => {
                        let _ = host::reveal_view(VIEW);
                        self.dispatch_tts(&text, lang.as_deref(), accent.as_deref(), &cfg)?;
                        Ok(None)
                    }
                    PendingAction::TestConnection { engine } => {
                        let res = translate_with_engine_mode(
                            engine,
                            "Hello",
                            "Hello",
                            "en",
                            &cfg.primary_language,
                            &cfg,
                            &mut self.edge_token,
                            false,
                        )?;
                        let msg = format!(
                            "{} OK ({}ms): {}",
                            engine.short_label(),
                            res.latency_ms,
                            res.translation
                        );
                        let _ = host::post_view_message(
                            VIEW,
                            &json!({ "op": "test-connection-result", "ok": true, "message": msg })
                                .to_string(),
                        );
                        Ok(Some(msg))
                    }
                }
            }
            _ => Err(Error::NotFound(format!("command {id}"))),
        }
    }

    fn handle_events(&mut self, events: &[Event], ctx: &Invocation) {
        for event in events {
            match event {
                Event::WebSocketReady(ready) => {
                    if let Some(pending) = self.pending_tts.get_mut(&ready.connection) {
                        match tts::drain_pending_edge_tts(pending) {
                            Ok(Some(res)) => {
                                let finished = self.pending_tts.remove(&ready.connection);
                                let cfg = TranslationConfig::load();
                                match res {
                                    Ok(audio) => {
                                        let _ = host::post_view_message(
                                            VIEW,
                                            &json!({ "op": "tts-audio", "audio": audio })
                                                .to_string(),
                                        );
                                    }
                                    Err(_) => {
                                        if let Some(p) = finished {
                                            let fb = tts::fallback_tts(
                                                &p.text,
                                                &p.lang,
                                                &p.accent,
                                                &cfg,
                                            );
                                            let _ = host::post_view_message(
                                                VIEW,
                                                &json!({ "op": "tts-audio", "audio": fb })
                                                    .to_string(),
                                            );
                                        }
                                    }
                                }
                            }
                            Ok(None) => {}
                            Err(_) => {
                                if let Some(p) = self.pending_tts.remove(&ready.connection) {
                                    let cfg = TranslationConfig::load();
                                    let fb =
                                        tts::fallback_tts(&p.text, &p.lang, &p.accent, &cfg);
                                    let _ = host::post_view_message(
                                        VIEW,
                                        &json!({ "op": "tts-audio", "audio": fb }).to_string(),
                                    );
                                }
                            }
                        }
                    }
                }
                Event::Other { kind, data } => {
                    if kind == "settings-changed" {
                        config::clear_overrides_for_changed_keys(data);
                        let cfg = TranslationConfig::load();
                        refresh_status_menu(&cfg);
                        self.post_config_only(&cfg);
                    } else if kind == "content-tab-action"
                        && let Ok(act) = serde_json::from_str::<ContentTabAction>(data)
                    {
                        let cfg = TranslationConfig::load();
                        let zh = cfg.is_zh();
                        match act.action.as_str() {
                            "copy" => {
                                let reply = match host::copy_to_clipboard(&act.content) {
                                    Ok(()) => ContentTabReply {
                                        status: Some(
                                            if zh {
                                                "已复制到剪贴板"
                                            } else {
                                                "Copied to clipboard"
                                            }
                                            .into(),
                                        ),
                                        ..ContentTabReply::default()
                                    },
                                    Err(e) => ContentTabReply {
                                        error: Some(e.to_string()),
                                        ..ContentTabReply::default()
                                    },
                                };
                                let _ = host::reply_content_tab(&act.request_id, &reply);
                            }
                            "diff" => {
                                let left = self
                                    .content_tab_sources
                                    .get(&act.key)
                                    .map(|(orig, _)| orig.clone())
                                    .unwrap_or_default();
                                let reply = ContentTabReply {
                                    diff: Some(ContentDiff {
                                        left,
                                        right: act.content,
                                        left_label: Some(
                                            if zh { "原文" } else { "Original" }.into(),
                                        ),
                                        right_label: Some(
                                            if zh { "译文" } else { "Translated" }.into(),
                                        ),
                                        title: Some(
                                            if zh {
                                                "文档翻译对比"
                                            } else {
                                                "Translated Document Diff"
                                            }
                                            .into(),
                                        ),
                                    }),
                                    ..ContentTabReply::default()
                                };
                                let _ = host::reply_content_tab(&act.request_id, &reply);
                            }
                            "apply" => {
                                let target_path = self
                                    .content_tab_sources
                                    .get(&act.key)
                                    .map(|(_, p)| p.clone());
                                let applied = if let (Some(path), Some(editor)) =
                                    (target_path, ctx.editor())
                                    && editor.path() == path
                                    && let Ok(doc) = editor.document()
                                {
                                    let end = openidea_plugin::text::position(
                                        &doc.text,
                                        doc.text.len(),
                                    );
                                    editor.apply_edits(
                                        doc.version,
                                        &[TextEdit {
                                            range: Range::new(Position::new(0, 0), end),
                                            text: act.content.clone(),
                                        }],
                                    )
                                } else {
                                    Err(Error::Invalid(
                                        "Open the original source file in the editor to apply"
                                            .into(),
                                    ))
                                };
                                let reply = match applied {
                                    Ok(_) => ContentTabReply {
                                        saved: Some(true),
                                        status: Some(
                                            if zh {
                                                "已应用到原编辑器"
                                            } else {
                                                "Applied to editor"
                                            }
                                            .into(),
                                        ),
                                        ..ContentTabReply::default()
                                    },
                                    Err(e) => ContentTabReply {
                                        error: Some(e.to_string()),
                                        ..ContentTabReply::default()
                                    },
                                };
                                let _ = host::reply_content_tab(&act.request_id, &reply);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

impl Views for TranslationPlugin {
    fn render(
        &mut self,
        view: &str,
        _viewport: Option<Viewport>,
        _ctx: &Invocation,
    ) -> Result<View, Error> {
        if view != VIEW {
            return Err(Error::NotFound(view.into()));
        }
        Ok(View::message("Translation UI renders inside its web view."))
    }

    fn handle(&mut self, view: &str, event: ViewEvent, ctx: &Invocation) -> Result<(), Error> {
        if view != VIEW {
            return Err(Error::NotFound(view.into()));
        }
        let ViewEvent::Message(raw) = event else {
            return Ok(());
        };
        let Ok(payload) = serde_json::from_str::<Value>(&raw) else {
            return Ok(());
        };
        let op = payload.get("op").and_then(Value::as_str).unwrap_or("");
        let cfg = TranslationConfig::load();

        match op {
            "ready" => {
                self.post_full_state(&cfg);
            }
            "translate" => {
                let raw_input = payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                if raw_input.is_empty() {
                    return Ok(());
                }
                let text = apply_ignore_regexp(&raw_input, &cfg.ignore_regexp);
                if text.trim().is_empty() {
                    return Ok(());
                }
                let split = payload
                    .get("splitWords")
                    .and_then(Value::as_bool)
                    .unwrap_or(cfg.auto_split_words);
                let query = if split {
                    split_identifier_with_separators(&text, &cfg.separators)
                        .unwrap_or_else(|| text.clone())
                } else {
                    text.clone()
                };
                let src = payload
                    .get("sourceLang")
                    .and_then(Value::as_str)
                    .unwrap_or(&cfg.source_language)
                    .to_owned();
                let tgt = payload
                    .get("targetLang")
                    .and_then(Value::as_str)
                    .unwrap_or(&cfg.target_language)
                    .to_owned();
                if tgt != "auto" {
                    config::record_last_target_language(&tgt);
                }
                let engine = payload
                    .get("engine")
                    .and_then(Value::as_str)
                    .and_then(EngineId::parse)
                    .unwrap_or(cfg.engine);

                match self.translate_cached_or_live(&raw_input, &query, &src, &tgt, engine, &cfg) {
                    Ok(res) => {
                        self.post_result_to_view(&res, &cfg);
                        if payload
                            .get("autoPlay")
                            .and_then(Value::as_bool)
                            .unwrap_or(cfg.tts_auto_play)
                            && query.chars().count() <= 80
                        {
                            let _ = self.dispatch_tts(&query, Some(&res.source_lang), None, &cfg);
                        }
                    }
                    Err(Error::Denied(cap)) => {
                        self.post_error_to_view(op, &Error::Denied(cap.clone()));
                        self.notify_permission_retry(
                            PendingAction::Translate {
                                raw_text: raw_input,
                                query_text: query,
                                source_pref: src,
                                target_pref: tgt,
                                engine,
                            },
                            &cap,
                            &cfg,
                        );
                    }
                    Err(err) => self.post_error_to_view(op, &err),
                }
            }
            "compare-engines" => {
                let raw_input = payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                if raw_input.is_empty() {
                    return Ok(());
                }
                let text = apply_ignore_regexp(&raw_input, &cfg.ignore_regexp);
                let split = payload
                    .get("splitWords")
                    .and_then(Value::as_bool)
                    .unwrap_or(cfg.auto_split_words);
                let query = if split {
                    split_identifier_with_separators(&text, &cfg.separators)
                        .unwrap_or_else(|| text.clone())
                } else {
                    text.clone()
                };
                let src = payload
                    .get("sourceLang")
                    .and_then(Value::as_str)
                    .unwrap_or(&cfg.source_language)
                    .to_owned();
                let tgt = payload
                    .get("targetLang")
                    .and_then(Value::as_str)
                    .unwrap_or(&cfg.target_language)
                    .to_owned();
                let snap = cfg.snapshot();
                let mut rows = Vec::new();
                for st in &snap.engines {
                    if !st.configured {
                        continue;
                    }
                    let Some(eng) = EngineId::parse(st.id) else {
                        continue;
                    };
                    match self.translate_cached_or_live_ext(
                        &raw_input, &query, &src, &tgt, eng, &cfg, false, false,
                    ) {
                        Ok(r) => rows.push(json!({
                            "engine": st.id,
                            "name": st.name,
                            "ok": true,
                            "translation": r.translation,
                            "latencyMs": r.latency_ms,
                            "fromCache": r.from_cache,
                            "sourceLang": r.source_lang,
                            "targetLang": r.target_lang,
                        })),
                        Err(err) => rows.push(json!({
                            "engine": st.id,
                            "name": st.name,
                            "ok": false,
                            "error": err.to_string(),
                        })),
                    }
                }
                let _ = host::post_view_message(
                    VIEW,
                    &json!({
                        "op": "compare-results",
                        "originalText": raw_input,
                        "queryText": query,
                        "results": rows,
                    })
                    .to_string(),
                );
            }
            "tts" => {
                let text = payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                let lang = payload.get("lang").and_then(Value::as_str).map(str::to_owned);
                let accent = payload
                    .get("accent")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let tts_override = payload
                    .get("ttsEngine")
                    .and_then(Value::as_str)
                    .and_then(TtsEngineId::parse);
                let mut eff_cfg = cfg.clone();
                if let Some(te) = tts_override {
                    eff_cfg.tts_engine = te;
                }
                match self.dispatch_tts(&text, lang.as_deref(), accent.as_deref(), &eff_cfg) {
                    Ok(()) => {}
                    Err(Error::Denied(cap)) => {
                        self.post_error_to_view(op, &Error::Denied(cap.clone()));
                        self.notify_permission_retry(
                            PendingAction::Tts { text, lang, accent },
                            &cap,
                            &cfg,
                        );
                    }
                    Err(err) => self.post_error_to_view(op, &err),
                }
            }
            "translate-clipboard" => {
                if let Some(txt) = payload.get("text").and_then(Value::as_str) {
                    let trimmed = txt.trim();
                    if !trimmed.is_empty() {
                        let cleaned = apply_ignore_regexp(trimmed, &cfg.ignore_regexp);
                        let query = if cfg.auto_split_words {
                            split_identifier_with_separators(&cleaned, &cfg.separators)
                                .unwrap_or_else(|| cleaned.clone())
                        } else {
                            cleaned
                        };
                        match self.translate_cached_or_live(
                            trimmed,
                            &query,
                            &cfg.source_language,
                            &cfg.target_language,
                            cfg.engine,
                            &cfg,
                        ) {
                            Ok(res) => self.post_result_to_view(&res, &cfg),
                            Err(err) => self.post_error_to_view(op, &err),
                        }
                        return Ok(());
                    }
                }
                match self.handle_translate_clipboard(&cfg) {
                    Ok(_) => {}
                    Err(Error::Denied(_)) => {
                        let zh = cfg.is_zh();
                        let _ = host::notify_with_actions(
                            NotificationLevel::Info,
                            if zh { "需要从命令读取剪贴板" } else { "Read Clipboard via Command" },
                            if zh {
                                "出于安全限制，插件需通过命令按钮读取系统剪贴板。"
                            } else {
                                "Clipboard reads require a user command action. Click below to translate clipboard."
                            },
                            &[NotificationAction::new(
                                if zh { "翻译剪贴板" } else { "Translate Clipboard" },
                                "translate-clipboard",
                            )],
                        );
                    }
                    Err(err) => self.post_error_to_view(op, &err),
                }
            }
            "grab-selection" => match self.extract_from_active_editor(ctx, &cfg) {
                Ok((editor, _doc, target)) => {
                    let _ = host::post_view_message(
                        VIEW,
                        &json!({
                            "op": "editor-selection",
                            "text": target.raw_text,
                            "queryText": target.query_text,
                            "path": editor.path(),
                            "language": editor.language(),
                        })
                        .to_string(),
                    );
                    if payload.get("translate").and_then(Value::as_bool).unwrap_or(false) {
                        match self.translate_cached_or_live(
                            &target.raw_text,
                            &target.query_text,
                            &cfg.source_language,
                            &cfg.target_language,
                            cfg.engine,
                            &cfg,
                        ) {
                            Ok(res) => self.post_result_to_view(&res, &cfg),
                            Err(err) => self.post_error_to_view("translate", &err),
                        }
                    }
                }
                Err(err) => self.post_error_to_view(op, &err),
            },
            "replace-in-editor" => {
                let replacement = payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if replacement.is_empty() {
                    return Ok(());
                }
                let result = (|| -> Result<(), Error> {
                    let editor = ctx
                        .editor()
                        .ok_or_else(|| Error::Invalid("No text editor is focused".into()))?;
                    let doc = editor.document()?;
                    let selections = editor.selections()?;
                    let sel = selections.first().copied().unwrap_or_default();
                    let range = if !sel.is_empty() {
                        sel
                    } else if let Some(last) = &self.last_target
                        && last.path == editor.path()
                        && last.version == doc.version
                    {
                        last.range
                    } else {
                        extract_editor_target_with_options(
                            &doc.text,
                            sel,
                            &cfg.auto_selection_mode,
                            cfg.auto_split_words,
                            &cfg.separators,
                            &cfg.ignore_regexp,
                        )
                        .ok_or_else(|| Error::Invalid("No selection or word to replace".into()))?
                        .range
                    };
                    let final_text = if cfg.keep_format {
                        let raw_slice = doc.text_in(range).unwrap_or("");
                        let (p, _core, s) = preserve_format_wrapper(raw_slice);
                        format!("{p}{replacement}{s}")
                    } else {
                        replacement.clone()
                    };
                    editor.apply_edits(
                        doc.version,
                        &[TextEdit {
                            range,
                            text: final_text,
                        }],
                    )?;
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        let _ = host::post_view_message(
                            VIEW,
                            &json!({
                                "op": "toast",
                                "message": if cfg.is_zh() { "已替换编辑器内容" } else { "Replaced in editor" }
                            })
                            .to_string(),
                        );
                    }
                    Err(err) => self.post_error_to_view(op, &err),
                }
            }
            "set-engine" => {
                if let Some(eng) = payload
                    .get("engine")
                    .and_then(Value::as_str)
                    .and_then(EngineId::parse)
                {
                    let _ = config::set_engine_override(eng);
                    let updated = TranslationConfig::load();
                    refresh_status_menu(&updated);
                    self.post_config_only(&updated);
                }
            }
            "set-languages" => {
                let src = payload.get("sourceLang").and_then(Value::as_str);
                let tgt = payload.get("targetLang").and_then(Value::as_str);
                let _ = config::set_language_overrides(src, tgt);
                let updated = TranslationConfig::load();
                refresh_status_menu(&updated);
                self.post_config_only(&updated);
            }
            "swap-languages" => {
                let _ = self.run_command("swap-languages", ctx, None);
            }
            "toggle-auto-split" => {
                let next = payload
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(!cfg.auto_split_words);
                let _ = config::set_auto_split_override(next);
                let updated = TranslationConfig::load();
                self.post_config_only(&updated);
            }
            "toggle-instant" => {
                let next = payload
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(!cfg.instant_translation);
                let _ = config::set_instant_translation_override(next);
                let updated = TranslationConfig::load();
                self.post_config_only(&updated);
            }
            "set-doc-bilingual" => {
                let next = payload
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(!cfg.doc_bilingual_mode);
                let _ = config::set_doc_bilingual_override(next);
                let updated = TranslationConfig::load();
                self.post_config_only(&updated);
            }
            "set-tts-engine" => {
                if let Some(te) = payload
                    .get("ttsEngine")
                    .and_then(Value::as_str)
                    .and_then(TtsEngineId::parse)
                {
                    let _ = config::set_tts_engine_override(te);
                }
                if let Some(voice) = payload.get("edgeVoice").and_then(Value::as_str) {
                    let _ = config::set_edge_voice_override(voice);
                }
                let updated = TranslationConfig::load();
                self.post_config_only(&updated);
            }
            "copy" => {
                if let Some(text) = payload.get("text").and_then(Value::as_str) {
                    let _ = host::copy_to_clipboard(text);
                }
            }
            "wordbook-add" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct AddPayload {
                    word: String,
                    #[serde(default)]
                    source_lang: String,
                    #[serde(default)]
                    target_lang: String,
                    translation: String,
                    #[serde(default)]
                    phonetic: String,
                    #[serde(default)]
                    explains: Vec<String>,
                    #[serde(default)]
                    tags: Vec<String>,
                    #[serde(default)]
                    note: String,
                }
                if let Ok(req) = serde_json::from_value::<AddPayload>(payload.clone())
                    && !req.word.trim().is_empty()
                {
                    let entry = WordBookEntry {
                        id: String::new(),
                        word: req.word.trim().to_owned(),
                        source_lang: if req.source_lang.is_empty() {
                            "auto".into()
                        } else {
                            req.source_lang
                        },
                        target_lang: if req.target_lang.is_empty() {
                            cfg.primary_language.clone()
                        } else {
                            req.target_lang
                        },
                        translation: req.translation.trim().to_owned(),
                        phonetic: req.phonetic,
                        explains: req.explains,
                        tags: req.tags,
                        note: req.note,
                        source_file: ctx
                            .editor()
                            .map(|e| e.path().rsplit(['/', '\\']).next().unwrap_or("").to_owned())
                            .unwrap_or_default(),
                        created_at_ms: unix_millis_now(),
                        review_count: 0,
                        mastered: false,
                    };
                    if let Ok((list, _, _)) = upsert_wordbook_entry(entry) {
                        let _ = host::post_view_message(
                            VIEW,
                            &json!({ "op": "wordbook-sync", "wordbook": list }).to_string(),
                        );
                    }
                }
            }
            "wordbook-update" => {
                let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
                let mut list = load_wordbook();
                if let Some(entry) = list.iter_mut().find(|e| e.id == id) {
                    if let Some(t) = payload.get("translation").and_then(Value::as_str) {
                        entry.translation = t.trim().to_owned();
                    }
                    if let Some(p) = payload.get("phonetic").and_then(Value::as_str) {
                        entry.phonetic = p.trim().to_owned();
                    }
                    if let Some(note) = payload.get("note").and_then(Value::as_str) {
                        entry.note = note.trim().to_owned();
                    }
                    if let Some(tags) = payload.get("tags").and_then(Value::as_array) {
                        entry.tags = tags
                            .iter()
                            .filter_map(Value::as_str)
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                    if let Some(mastered) = payload.get("mastered").and_then(Value::as_bool) {
                        entry.mastered = mastered;
                    }
                    let _ = save_wordbook(&list);
                }
                let _ = host::post_view_message(
                    VIEW,
                    &json!({ "op": "wordbook-sync", "wordbook": list }).to_string(),
                );
            }
            "wordbook-review" => {
                let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
                let mastered = payload.get("mastered").and_then(Value::as_bool);
                let mut list = load_wordbook();
                if let Some(entry) = list.iter_mut().find(|e| e.id == id) {
                    entry.review_count = entry.review_count.saturating_add(1);
                    if let Some(m) = mastered {
                        entry.mastered = m;
                    }
                    let _ = save_wordbook(&list);
                }
                let _ = host::post_view_message(
                    VIEW,
                    &json!({ "op": "wordbook-sync", "wordbook": list }).to_string(),
                );
            }
            "wordbook-delete" => {
                let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
                let mut list = load_wordbook();
                list.retain(|e| e.id != id);
                let _ = save_wordbook(&list);
                let _ = host::post_view_message(
                    VIEW,
                    &json!({ "op": "wordbook-sync", "wordbook": list }).to_string(),
                );
            }
            "wordbook-rename-tag" => {
                let old_tag = payload.get("oldTag").and_then(Value::as_str).unwrap_or("");
                let new_tag = payload.get("newTag").and_then(Value::as_str).unwrap_or("");
                match rename_wordbook_tag(old_tag, new_tag) {
                    Ok((list, changed)) => {
                        let _ = host::post_view_message(
                            VIEW,
                            &json!({
                                "op": "wordbook-sync",
                                "wordbook": list,
                                "toastMessage": if cfg.is_zh() {
                                    format!("已重命名标签（更新 {changed} 条）")
                                } else {
                                    format!("Renamed tag on {changed} entry(ies)")
                                }
                            })
                            .to_string(),
                        );
                    }
                    Err(err) => self.post_error_to_view(op, &err),
                }
            }
            "wordbook-delete-tag" => {
                let tag = payload.get("tag").and_then(Value::as_str).unwrap_or("");
                match delete_wordbook_tag(tag) {
                    Ok((list, changed)) => {
                        let _ = host::post_view_message(
                            VIEW,
                            &json!({
                                "op": "wordbook-sync",
                                "wordbook": list,
                                "toastMessage": if cfg.is_zh() {
                                    format!("已移除标签（更新 {changed} 条）")
                                } else {
                                    format!("Removed tag from {changed} entry(ies)")
                                }
                            })
                            .to_string(),
                        );
                    }
                    Err(err) => self.post_error_to_view(op, &err),
                }
            }
            "word-of-the-day" => {
                let list = load_wordbook();
                let entry = pick_word_of_the_day(&list).cloned();
                let _ = host::post_view_message(
                    VIEW,
                    &json!({ "op": "word-of-the-day", "entry": entry }).to_string(),
                );
            }
            "wordbook-export" => {
                let format = payload
                    .get("format")
                    .and_then(Value::as_str)
                    .unwrap_or("csv");
                let list = load_wordbook();
                match export_wordbook(&list, format) {
                    Ok((filename, content)) => {
                        let _ = host::post_view_message(
                            VIEW,
                            &json!({
                                "op": "export-data",
                                "filename": filename,
                                "format": format,
                                "content": content,
                            })
                            .to_string(),
                        );
                    }
                    Err(err) => self.post_error_to_view(op, &err),
                }
            }
            "wordbook-import" => {
                let raw_data = payload.get("content").and_then(Value::as_str).unwrap_or("");
                match import_wordbook_text(raw_data) {
                    Ok((list, count)) => {
                        let _ = host::post_view_message(
                            VIEW,
                            &json!({
                                "op": "wordbook-sync",
                                "wordbook": list,
                                "importedCount": count,
                            })
                            .to_string(),
                        );
                    }
                    Err(err) => self.post_error_to_view(op, &err),
                }
            }
            "history-clear" => {
                let _ = save_json::<Vec<HistoryItem>>(storage::KEY_HISTORY, &Vec::new());
                let _ = host::post_view_message(
                    VIEW,
                    &json!({ "op": "history-sync", "history": [] }).to_string(),
                );
            }
            "history-delete" => {
                let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
                let mut history = load_history();
                history.retain(|h| h.id != id);
                let _ = save_json(storage::KEY_HISTORY, &history);
                let _ = host::post_view_message(
                    VIEW,
                    &json!({ "op": "history-sync", "history": history }).to_string(),
                );
            }
            "doc-translate-selection" => {
                if let Err(err) = self.handle_translate_doc_command(ctx, &cfg) {
                    self.post_error_to_view(op, &err);
                }
            }
            "doc-translate-comments" => {
                let result = (|| -> Result<(), Error> {
                    let editor = ctx
                        .editor()
                        .ok_or_else(|| Error::Invalid("No text editor is focused".into()))?;
                    let doc = editor.document()?;
                    let blocks = extract_all_comments(&doc.text, 60);
                    if blocks.is_empty() {
                        return Err(Error::Invalid(
                            "No comment blocks found in active file".into(),
                        ));
                    }
                    let items = self.translate_comment_blocks(&blocks, &cfg)?;
                    let _ = host::post_view_message(
                        VIEW,
                        &json!({
                            "op": "doc-comments-list",
                            "path": editor.path(),
                            "items": items,
                        })
                        .to_string(),
                    );
                    Ok(())
                })();
                if let Err(err) = result {
                    self.post_error_to_view(op, &err);
                }
            }
            "doc-toggle-annotations" => {
                if let Err(err) = self.handle_toggle_doc_annotations(ctx, &cfg) {
                    self.post_error_to_view(op, &err);
                }
            }
            "doc-clear-annotations" => {
                for path in self.annotated_files.drain() {
                    let _ = host::set_decorations(&path, DECORATION_SET_COMMENTS, &[]);
                }
                let _ = host::clear_decorations(DECORATION_SET_COMMENTS);
                let _ = host::post_view_message(
                    VIEW,
                    &json!({ "op": "doc-annotations-status", "active": false }).to_string(),
                );
            }
            "doc-translate-file" => {
                if let Err(err) = self.handle_translate_document(ctx, &cfg) {
                    self.post_error_to_view(op, &err);
                }
            }
            "doc-open-diff" => {
                let left = payload.get("original").and_then(Value::as_str).unwrap_or("");
                let right = payload
                    .get("translation")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let file_name = payload.get("fileName").and_then(Value::as_str);
                if let Err(err) = host::open_diff(
                    &ContentDiff {
                        left: left.to_owned(),
                        right: right.to_owned(),
                        left_label: Some(if cfg.is_zh() { "原文" } else { "Original" }.into()),
                        right_label: Some(
                            if cfg.is_zh() { "译文" } else { "Translated" }.into(),
                        ),
                        title: Some(
                            if cfg.is_zh() {
                                "文档翻译对比"
                            } else {
                                "Document Translation Diff"
                            }
                            .into(),
                        ),
                    },
                    file_name,
                ) {
                    self.post_error_to_view(op, &err);
                }
            }
            "cache-clear" => {
                self.ensure_cache(cfg.cache_ttl_hours);
                let _ = storage::clear_cache(&mut self.cache);
                self.post_config_only(&cfg);
            }
            "test-connection" => {
                let engine = payload
                    .get("engine")
                    .and_then(Value::as_str)
                    .and_then(EngineId::parse)
                    .unwrap_or(cfg.engine);
                match translate_with_engine_mode(
                    engine,
                    "Hello",
                    "Hello",
                    "en",
                    &cfg.primary_language,
                    &cfg,
                    &mut self.edge_token,
                    false,
                ) {
                    Ok(res) => {
                        let msg = if cfg.is_zh() {
                            format!(
                                "{} 连接成功 ({}ms): Hello → {}",
                                engine.display_name(true),
                                res.latency_ms,
                                res.translation
                            )
                        } else {
                            format!(
                                "{} OK ({}ms): Hello -> {}",
                                engine.display_name(false),
                                res.latency_ms,
                                res.translation
                            )
                        };
                        let _ = host::post_view_message(
                            VIEW,
                            &json!({ "op": "test-connection-result", "ok": true, "message": msg })
                                .to_string(),
                        );
                    }
                    Err(Error::Denied(cap)) => {
                        self.post_error_to_view(op, &Error::Denied(cap.clone()));
                        self.notify_permission_retry(
                            PendingAction::TestConnection { engine },
                            &cap,
                            &cfg,
                        );
                    }
                    Err(err) => {
                        let _ = host::post_view_message(
                            VIEW,
                            &json!({
                                "op": "test-connection-result",
                                "ok": false,
                                "message": err.to_string()
                            })
                            .to_string(),
                        );
                    }
                }
            }
            other => {
                host::log(LogLevel::Debug, &format!("Unhandled view message op: {other}"));
            }
        }
        Ok(())
    }
}

impl HoverProvider for TranslationPlugin {
    fn hover(&mut self, doc: &HoverDocument, at: Position) -> Option<Hover> {
        let cfg = TranslationConfig::load();
        if !cfg.show_hover_translation {
            return None;
        }
        let target = extract_editor_target_with_options(
            &doc.text,
            Range::new(at, at),
            &cfg.auto_selection_mode,
            cfg.auto_split_words,
            &cfg.separators,
            &cfg.ignore_regexp,
        )?;
        self.ensure_cache(cfg.cache_ttl_hours);
        let wordbook = load_wordbook();
        let split_opt = if target.was_split {
            Some(target.query_text.as_str())
        } else {
            None
        };

        if let Some(wb) = find_wordbook(&wordbook, &target.raw_text, split_opt) {
            let mut md = format!("**{}**", wb.word);
            if !wb.phonetic.is_empty() {
                md.push_str(&format!(" `/{}/`", wb.phonetic.trim_matches('/')));
            }
            md.push_str(" ★\n\n");
            md.push_str(&format!("**{}**", wb.translation));
            if !wb.explains.is_empty() {
                md.push_str("\n\n");
                for ex in wb.explains.iter().take(4) {
                    md.push_str(&format!("- {ex}\n"));
                }
            }
            if !wb.note.is_empty() {
                md.push_str(&format!("\n> {}", wb.note));
            }
            return Some(Hover::new(md).range(target.range));
        }

        let cached = lookup_cache_by_term(&self.cache, &target.raw_text, split_opt)?;
        let mut md = format!("**{}**", target.raw_text);
        if target.was_split {
            md.push_str(&format!(" *({})*", target.query_text));
        }
        if let Some(p) = cached
            .phonetics
            .us
            .as_deref()
            .or(cached.phonetics.uk.as_deref())
            .or(cached.phonetics.general.as_deref())
        {
            md.push_str(&format!(" `/{}/`", p.trim_matches('/')));
        }
        md.push_str(&format!(" · *{}*\n\n**{}**", cached.engine, cached.translation));
        for entry in cached.dict.iter().take(3) {
            let line = if !entry.explains.is_empty() {
                entry.explains.join("; ")
            } else {
                entry.terms.join(", ")
            };
            if !line.is_empty() {
                if entry.pos.is_empty() {
                    md.push_str(&format!("\n- {line}"));
                } else {
                    md.push_str(&format!("\n- *{}* {line}", entry.pos));
                }
            }
        }
        Some(Hover::new(md).range(target.range))
    }
}

impl Providers for TranslationPlugin {
    fn code_actions(&mut self, doc: &ProviderDocument, selection: Range) -> Vec<CodeAction> {
        let cfg = TranslationConfig::load();
        let Some(target) = extract_editor_target_with_options(
            &doc.text,
            selection,
            &cfg.auto_selection_mode,
            cfg.auto_split_words,
            &cfg.separators,
            &cfg.ignore_regexp,
        ) else {
            return Vec::new();
        };
        if target.raw_text.len() > 2_000 {
            return Vec::new();
        }
        let has_non_ascii = !target.raw_text.is_ascii();
        let has_multi_part = target.was_split || target.raw_text.contains(' ');
        if selection.is_empty() && !has_non_ascii && !has_multi_part {
            return Vec::new();
        }
        let data = json!({
            "raw": target.raw_text,
            "query": target.query_text,
            "range": target.range,
        })
        .to_string();
        let title = if cfg.is_zh() {
            format!("翻译并替换 \"{}\"", truncate_label(&target.raw_text, 24))
        } else {
            format!("Translate & Replace \"{}\"", truncate_label(&target.raw_text, 24))
        };
        vec![CodeAction {
            title,
            kind: Some("refactor".into()),
            preferred: false,
            edits: Vec::new(),
            data: Some(data),
        }]
    }

    fn resolve_code_action(
        &mut self,
        doc: &ProviderDocument,
        mut action: CodeAction,
    ) -> Result<CodeAction, Error> {
        #[derive(Deserialize)]
        struct ActionData {
            raw: String,
            query: String,
            range: Range,
        }
        let data_str = action
            .data
            .as_deref()
            .ok_or_else(|| Error::Invalid("Missing code action data".into()))?;
        let data: ActionData = serde_json::from_str(data_str)
            .map_err(|e| Error::Invalid(format!("Invalid code action data: {e}")))?;

        let cfg = TranslationConfig::load();
        let (prefix, core_raw, suffix) = if cfg.keep_format {
            let (p, c, s) = preserve_format_wrapper(&data.raw);
            (p.to_owned(), c.to_owned(), s.to_owned())
        } else {
            (String::new(), data.raw.clone(), String::new())
        };
        let query = if cfg.keep_format && core_raw != data.raw {
            let cleaned = apply_ignore_regexp(&core_raw, &cfg.ignore_regexp);
            if cfg.auto_split_words {
                split_identifier_with_separators(&cleaned, &cfg.separators).unwrap_or(cleaned)
            } else {
                cleaned
            }
        } else {
            data.query.clone()
        };

        let res = self.translate_cached_or_live(
            &core_raw,
            &query,
            &cfg.source_language,
            &cfg.target_language,
            cfg.engine,
            &cfg,
        )?;
        let candidates = generate_replace_candidates_ext(
            &res.translation,
            &res.alternatives,
            &cfg.preferred_naming_style,
            cfg.strip_articles_in_identifier,
            &cfg.separators,
        );
        if candidates.is_empty() {
            return Err(Error::Cancelled);
        }
        let chosen = if cfg.auto_replace || candidates.len() == 1 {
            candidates[0].text.clone()
        } else {
            let items: Vec<PickItem> = candidates
                .iter()
                .map(|c| PickItem::new(&c.text, &c.text).description(&c.style))
                .collect();
            let title = if cfg.is_zh() {
                format!("替换 \"{}\" 为：", core_raw)
            } else {
                format!("Replace \"{}\" with:", core_raw)
            };
            match host::pick(&title, &items)? {
                Some(choice) => choice,
                None => return Err(Error::Cancelled),
            }
        };
        action.edits = vec![(
            doc.path.clone(),
            vec![TextEdit {
                range: data.range,
                text: format!("{prefix}{chosen}{suffix}"),
            }],
        )];
        Ok(action)
    }
}

impl SearchProvider for TranslationPlugin {
    fn search(
        &mut self,
        category: &str,
        query: &str,
        limit: u32,
        _ctx: &Invocation,
    ) -> Result<SearchPage, Error> {
        if category != "wordbook" {
            return Ok(SearchPage::default());
        }
        let cfg = TranslationConfig::load();
        let max = (limit as usize).clamp(1, 200);
        let q = query.trim().to_ascii_lowercase();
        let wordbook = load_wordbook();
        let mut scored: Vec<(u8, SearchItem)> = Vec::new();
        let mut seen = HashSet::new();

        for entry in &wordbook {
            let lw = entry.word.to_ascii_lowercase();
            let lt = entry.translation.to_ascii_lowercase();
            let ln = entry.note.to_ascii_lowercase();
            let tags_joined = entry.tags.join(", ").to_ascii_lowercase();

            let rank = if q.is_empty() {
                2
            } else if lw == q {
                0
            } else if lw.starts_with(&q) {
                1
            } else if lw.contains(&q)
                || lt.contains(&q)
                || tags_joined.contains(&q)
                || ln.contains(&q)
            {
                2
            } else {
                continue;
            };
            seen.insert(lw);
            let mut desc = entry.translation.clone();
            if !entry.phonetic.is_empty() {
                desc = format!("[{}] {}", entry.phonetic.trim_matches('/'), desc);
            }
            if !entry.tags.is_empty() {
                desc.push_str(&format!(" · #{}", entry.tags.join(" #")));
            }
            scored.push((
                rank,
                SearchItem {
                    id: entry.id.clone(),
                    title: entry.word.clone(),
                    description: Some(desc),
                    target: SearchTarget::Command {
                        id: "open-wordbook".into(),
                        arg: Some(entry.word.clone()),
                    },
                },
            ));
        }

        if !q.is_empty() {
            self.ensure_cache(cfg.cache_ttl_hours);
            for c in &self.cache {
                if seen.contains(&c.lower_original) {
                    continue;
                }
                if c.lower_original.contains(&q)
                    || c.result.translation.to_ascii_lowercase().contains(&q)
                {
                    seen.insert(c.lower_original.clone());
                    scored.push((
                        3,
                        SearchItem {
                            id: format!("cache:{}", c.lower_original),
                            title: c.result.original_text.clone(),
                            description: Some(format!("{} (cached)", c.result.translation)),
                            target: SearchTarget::Command {
                                id: "open-panel".into(),
                                arg: None,
                            },
                        },
                    ));
                }
            }
        }

        scored.sort_by_key(|(rank, _)| *rank);
        let has_more = scored.len() > max;
        let items: Vec<SearchItem> = scored.into_iter().take(max).map(|(_, it)| it).collect();
        Ok(SearchPage { items, has_more })
    }
}

fn truncate_label(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

register_plugin!(TranslationPlugin, views, hover, providers, search);

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::HttpResponse;
    use openidea_plugin::mock::MockHost;

    fn setup_host() -> MockHost {
        MockHost::install().with_capabilities(&[
            "editor.read",
            "editor.write",
            "clipboard",
            "clipboard.read",
            "storage",
            "events",
            "project.read",
            "secrets",
            "net:translate.googleapis.com",
            "net:dict.youdao.com",
        ])
    }

    #[test]
    fn activate_subscribes_events_and_sets_status_menu() {
        let host = setup_host();
        let mut plugin = TranslationPlugin::default();
        plugin.activate(&host.context("ruyimg.translation")).unwrap();
        assert!(host.subscriptions().contains(&EventKind::SettingsChanged));
        assert!(host.subscriptions().contains(&EventKind::ContentTabAction));
        assert!(host.subscriptions().contains(&EventKind::WebsocketReady));
        assert!(host.status_items().contains_key(STATUS_ID));
        assert!(!host.status_menu(STATUS_ID).is_empty());
    }

    #[test]
    fn translate_inclusive_exclusive_and_and_copy_commands_work() {
        let host = setup_host();
        host.on_fetch(|req| {
            let word = if req.url.contains("Profile") && !req.url.contains("user") {
                "个人资料"
            } else {
                "用户个人资料"
            };
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: serde_json::json!({
                    "sentences": [{ "trans": word }],
                    "src": "en"
                })
                .to_string()
                .into_bytes(),
            })
        });
        host.open_editor("/proj/src/lib.rs", "rust", "let userProfile = 1;", &[(0, 10, 0, 10)]);
        let mut plugin = TranslationPlugin::default();

        plugin.run_command("translate-exclusive", &host.invocation(), None).unwrap();
        assert_eq!(plugin.last_result.as_ref().unwrap().translation, "个人资料");

        plugin.run_command("translate-inclusive", &host.invocation(), None).unwrap();
        assert_eq!(plugin.last_result.as_ref().unwrap().translation, "用户个人资料");

        let msg = plugin.run_command("translate-and-copy", &host.invocation(), None).unwrap().unwrap();
        assert!(msg.contains("用户个人资料"));
        assert_eq!(host.clipboard().as_deref(), Some("用户个人资料"));
    }

    #[test]
    fn translate_and_replace_updates_editor_text_and_supports_auto_replace_and_multicursor() {
        let host = setup_host();
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"sentences":[{"trans":"user authentication service"}],"src":"zh-CN"}"#
                    .to_vec(),
            })
        });
        host.open_editor("/proj/src/main.rs", "rust", "let 用户认证服务 = 1;", &[(0, 4, 0, 10)]);
        host.answer_next_prompt(Some(&["userAuthenticationService"]));

        let mut plugin = TranslationPlugin::default();
        plugin.run_command("translate-and-replace", &host.invocation(), None).unwrap();
        assert_eq!(host.editor_text(), "let userAuthenticationService = 1;");

        // Auto-replace + multi-cursor + keepFormat quotes
        host.set_setting("autoReplace", "true");
        host.set_setting("preferredNamingStyle", "\"snake_case\"");
        host.open_editor(
            "/proj/src/multi.rs",
            "rust",
            "let a = \"用户认证\";\nlet b = \"用户认证\";\n",
            &[(0, 8, 0, 14), (1, 8, 1, 14)],
        );
        plugin.run_command("translate-and-replace", &host.invocation(), None).unwrap();
        assert_eq!(
            host.editor_text(),
            "let a = \"user_authentication_service\";\nlet b = \"user_authentication_service\";\n"
        );
    }

    #[test]
    fn word_of_the_day_and_clipboard_and_startup_wotd_work() {
        let host = setup_host();
        upsert_wordbook_entry(WordBookEntry {
            id: "w-wotd".into(),
            word: "idempotent".into(),
            source_lang: "en".into(),
            target_lang: "zh-CN".into(),
            translation: "幂等的".into(),
            phonetic: "/ˌaɪdɛmˈpoʊtənt/".into(),
            explains: vec!["adj. 幂等的".into()],
            tags: vec![],
            note: String::new(),
            source_file: String::new(),
            created_at_ms: 1,
            review_count: 0,
            mastered: false,
        })
        .unwrap();

        host.set_setting("showWordsOnStartup", "true");
        let mut plugin = TranslationPlugin::default();
        plugin.activate(&host.context("ruyimg.translation")).unwrap();
        assert!(
            host.notifications_with_actions()
                .iter()
                .any(|n| n.notification.title.contains("idempotent"))
        );

        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: serde_json::json!({
                    "sentences": [{ "trans": "剪贴板文本" }],
                    "src": "en"
                })
                .to_string()
                .into_bytes(),
            })
        });
        host.set_clipboard(Some("clipboard text"), None);
        plugin.run_command("translate-clipboard", &host.invocation(), None).unwrap();
        assert_eq!(plugin.last_result.as_ref().unwrap().translation, "剪贴板文本");
    }

    #[test]
    fn websocket_ready_event_streams_edge_tts_and_falls_back_on_empty_close() {
        let host = MockHost::install().with_capabilities(&[
            "storage",
            "project.read",
            "net:speech.platform.bing.com",
            "net:dict.youdao.com",
        ]);
        host.set_project_root(Some("/proj"));
        host.set_setting("ttsEngine", "\"edge\"");
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: b"FALLBACK_YD".to_vec(),
            })
        });

        let mut plugin = TranslationPlugin::default();
        // 1) Normal Edge TTS WebSocket completion
        plugin
            .handle(
                VIEW,
                ViewEvent::Message(r#"{"op":"tts","text":"resilient","lang":"en"}"#.into()),
                &host.invocation(),
            )
            .unwrap();
        let ws1 = host.websockets().pop().unwrap();
        let hdr = b"Path:audio\r\n\r\n";
        let mut frame = Vec::new();
        frame.extend_from_slice(&(hdr.len() as u16).to_be_bytes());
        frame.extend_from_slice(hdr);
        frame.extend_from_slice(b"EDGE_AUDIO");
        host.push_websocket_message(ws1.id, &frame).unwrap();
        host.close_websocket(ws1.id).unwrap();
        plugin.handle_events(
            &[Event::WebSocketReady(openidea_plugin::WebSocketReady {
                plugin_id: "ruyimg.translation".into(),
                connection: ws1.id,
            })],
            &host.invocation(),
        );
        assert!(host
            .view_messages()
            .iter()
            .any(|(_, m)| m.contains("\"engine\":\"edge\"")));

        // 2) Empty close triggers automatic fallback to Youdao HTTP
        plugin
            .handle(
                VIEW,
                ViewEvent::Message(r#"{"op":"tts","text":"hello","lang":"en"}"#.into()),
                &host.invocation(),
            )
            .unwrap();
        let ws2 = host.websockets().into_iter().find(|s| s.id != ws1.id).unwrap();
        host.close_websocket(ws2.id).unwrap();
        plugin.handle_events(
            &[Event::WebSocketReady(openidea_plugin::WebSocketReady {
                plugin_id: "ruyimg.translation".into(),
                connection: ws2.id,
            })],
            &host.invocation(),
        );
        assert!(host
            .view_messages()
            .iter()
            .any(|(_, m)| m.contains("\"engine\":\"youdao\"")));
    }

    #[test]
    fn hover_reads_wordbook_and_cache_with_zero_network() {
        let host = setup_host();
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: r#"{"sentences":[{"trans":"获取用户资料"}],"src":"en"}"#.as_bytes().to_vec(),
            })
        });
        host.open_editor("/proj/src/lib.rs", "rust", "fn getUserProfile() {}", &[(0, 5, 0, 5)]);

        let mut plugin = TranslationPlugin::default();
        plugin.run_command("translate", &host.invocation(), None).unwrap();
        let reqs_before_hover = host.requests().len();

        let h = plugin
            .hover(
                &HoverDocument {
                    path: "/proj/src/lib.rs".into(),
                    language: "rust".into(),
                    version: 1,
                    text: "fn getUserProfile() {}".into(),
                },
                Position::new(0, 5),
            )
            .unwrap();
        assert!(h.markdown.contains("获取用户资料"));
        assert_eq!(host.requests().len(), reqs_before_hover);

        plugin.run_command("add-to-wordbook", &host.invocation(), None).unwrap();
        let h2 = plugin
            .hover(
                &HoverDocument {
                    path: "/proj/src/lib.rs".into(),
                    language: "rust".into(),
                    version: 1,
                    text: "fn getUserProfile() {}".into(),
                },
                Position::new(0, 5),
            )
            .unwrap();
        assert!(h2.markdown.contains('★'));
    }

    #[test]
    fn toggle_doc_annotations_sets_and_clears_decorations() {
        let host = setup_host();
        host.set_project_root(Some("/proj"));
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: r#"{"sentences":[{"trans":"初始化数据库连接池"}],"src":"en"}"#
                    .as_bytes()
                    .to_vec(),
            })
        });
        host.open_editor(
            "/proj/src/db.rs",
            "rust",
            "// Initialize database connection pool\nfn init() {}\n",
            &[(0, 0, 0, 0)],
        );

        let mut plugin = TranslationPlugin::default();
        plugin.run_command("toggle-doc-annotations", &host.invocation(), None).unwrap();
        let decs = host.decorations();
        let items = decs
            .get(&(DECORATION_SET_COMMENTS.into(), "/proj/src/db.rs".into()))
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].after.as_deref(), Some("  // 初始化数据库连接池"));

        plugin.run_command("toggle-doc-annotations", &host.invocation(), None).unwrap();
        let decs_after = host.decorations();
        let items_after = decs_after
            .get(&(DECORATION_SET_COMMENTS.into(), "/proj/src/db.rs".into()))
            .unwrap();
        assert!(items_after.is_empty());
    }

    #[test]
    fn code_action_search_and_compare_engines_work() {
        let host = setup_host();
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"sentences":[{"trans":"query user order"}],"src":"zh-CN"}"#.to_vec(),
            })
        });
        let mut plugin = TranslationPlugin::default();
        let pdoc = ProviderDocument {
            path: "/proj/src/svc.rs".into(),
            language: "rust".into(),
            version: 1,
            text: "fn 查询用户订单() {}".into(),
        };
        let actions = plugin.code_actions(&pdoc, Range::new(Position::new(0, 3), Position::new(0, 9)));
        assert_eq!(actions.len(), 1);
        host.answer_next_prompt(Some(&["queryUserOrder"]));
        let resolved = plugin.resolve_code_action(&pdoc, actions[0].clone()).unwrap();
        assert_eq!(resolved.edits[0].1[0].text, "queryUserOrder");

        plugin
            .handle(
                VIEW,
                ViewEvent::Message(
                    r#"{"op":"compare-engines","text":"cache","sourceLang":"en","targetLang":"zh-CN"}"#
                        .into(),
                ),
                &host.invocation(),
            )
            .unwrap();
        assert!(host
            .view_messages()
            .iter()
            .any(|(_, m)| m.contains("compare-results")));
    }

    #[test]
    fn webview_denied_host_notifies_and_retries_via_command() {
        let host = setup_host();
        host.set_setting("openaiBaseUrl", "\"https://api.deepseek.com/v1\"");
        host.set_secret("openai-api-key", "sk-ds");
        let mut plugin = TranslationPlugin::default();

        plugin
            .handle(
                VIEW,
                ViewEvent::Message(r#"{"op":"translate","text":"cache","engine":"openai","sourceLang":"en","targetLang":"zh-CN"}"#.into()),
                &host.invocation(),
            )
            .unwrap();
        assert_eq!(host.notifications_with_actions().len(), 1);
        assert_eq!(
            host.notifications_with_actions()[0].actions[0].command,
            "retry-pending-request"
        );

        let host2 = MockHost::install().with_capabilities(&[
            "storage",
            "secrets",
            "net:api.deepseek.com",
        ]);
        host2.set_setting("openaiBaseUrl", "\"https://api.deepseek.com/v1\"");
        host2.set_secret("openai-api-key", "sk-ds");
        host2.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: r#"{"choices":[{"message":{"content":"缓存"}}]}"#.as_bytes().to_vec(),
            })
        });
        plugin.run_command("retry-pending-request", &host2.invocation(), None).unwrap();
        assert!(host2.view_messages().iter().any(|(_, m)| m.contains("缓存")));
    }

    #[test]
    fn settings_changed_clears_matching_override_and_updates_status() {
        let host = setup_host();
        let mut plugin = TranslationPlugin::default();
        plugin.activate(&host.context("ruyimg.translation")).unwrap();
        plugin.run_command("switch-engine", &host.invocation(), Some("youdao")).unwrap();
        assert_eq!(TranslationConfig::load().engine, EngineId::Youdao);

        host.set_setting("engine", "\"google\"");
        plugin.handle_events(
            &[Event::Other {
                kind: "settings-changed".into(),
                data: r#"["ruyimg.translation.engine"]"#.into(),
            }],
            &host.invocation(),
        );
        assert_eq!(TranslationConfig::load().engine, EngineId::Google);
    }

    #[test]
    fn translate_doc_comment_bilingual_and_ipynb_work() {
        let host = setup_host();
        host.on_fetch(|req| {
            let decoded = if req.url.contains("Hello") {
                "你好"
            } else if req.url.contains("Fetches") {
                "获取 [[TT_0]] 用户配置。"
            } else if req.url.contains("NotebookTitle") {
                "# 笔记本标题"
            } else {
                "文档正文译文"
            };
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: serde_json::json!({
                    "sentences": [{ "trans": decoded }],
                    "src": "en"
                })
                .to_string()
                .into_bytes(),
            })
        });

        let mut plugin = TranslationPlugin::default();
        host.open_editor(
            "/proj/src/doc.rs",
            "rust",
            "/**\n * Fetches {@link User} profile.\n */\nfn get() {}\n",
            &[(1, 4, 1, 4)],
        );
        plugin.run_command("translate-doc", &host.invocation(), None).unwrap();
        assert!(host.revealed_views().contains(&VIEW.to_owned()));
        assert!(host
            .view_messages()
            .iter()
            .any(|(_, m)| m.contains("doc-comment-result") && m.contains("{@link User}")));

        // Jupyter .ipynb translates markdown cells only
        host.open_editor(
            "/proj/analysis.ipynb",
            "json",
            r##"{"cells":[{"cell_type":"markdown","source":["# NotebookTitle"]},{"cell_type":"code","source":["x = 1"]}],"nbformat":4}"##,
            &[(0, 0, 0, 0)],
        );
        plugin.run_command("translate-document", &host.invocation(), None).unwrap();
        let nb_msg = host
            .view_messages()
            .into_iter()
            .rev()
            .find(|(_, m)| m.contains("doc-file-result"))
            .unwrap()
            .1;
        assert!(nb_msg.contains("# 笔记本标题"));
        assert!(nb_msg.contains("x = 1"));
    }

    #[test]
    fn quick_translate_swap_clear_cache_and_test_connection_work() {
        let host = setup_host();
        host.on_fetch(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: serde_json::json!({
                    "sentences": [{ "trans": "你好" }],
                    "src": "en"
                })
                .to_string()
                .into_bytes(),
            })
        });
        host.answer_next_prompt(Some(&["hello world"]));

        let mut plugin = TranslationPlugin::default();
        plugin.run_command("quick-translate", &host.invocation(), None).unwrap();
        assert!(host.view_messages().iter().any(|(_, m)| m.contains("你好")));

        let msg = plugin
            .run_command("test-connection", &host.invocation(), None)
            .unwrap()
            .unwrap();
        assert!(msg.contains("Google"));

        plugin.run_command("swap-languages", &host.invocation(), None).unwrap();
        let cfg = TranslationConfig::load();
        assert_eq!(cfg.source_language, "zh-CN");
        assert_eq!(cfg.target_language, "en");

        let cleared = plugin
            .run_command("clear-cache", &host.invocation(), None)
            .unwrap()
            .unwrap();
        assert!(cleared.contains("cleared") || cleared.contains("清空"));
        assert!(load_cache().is_empty());
    }
}
