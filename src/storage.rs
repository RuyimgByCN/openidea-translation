use crate::engines::{TranslationResult, unix_millis_now};
use openidea_plugin::Error;
use openidea_plugin::host::storage;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const KEY_CACHE: &str = "cache:v1";
pub const KEY_WORDBOOK: &str = "wordbook:v1";
pub const KEY_HISTORY: &str = "history:v1";

const MAX_CACHE_ENTRIES: usize = 400;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntry {
    pub key: String,
    pub lower_original: String,
    pub lower_query: String,
    pub result: TranslationResult,
    pub last_used_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WordBookEntry {
    pub id: String,
    pub word: String,
    pub source_lang: String,
    pub target_lang: String,
    pub translation: String,
    #[serde(default)]
    pub phonetic: String,
    #[serde(default)]
    pub explains: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub source_file: String,
    pub created_at_ms: u64,
    #[serde(default)]
    pub review_count: u32,
    #[serde(default)]
    pub mastered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: String,
    pub engine: String,
    pub original_text: String,
    pub query_text: String,
    pub translation: String,
    pub source_lang: String,
    pub target_lang: String,
    pub timestamp_ms: u64,
}

pub fn load_json<T: for<'de> Deserialize<'de>>(key: &str) -> Option<T> {
    let raw = storage::get(key).ok().flatten()?;
    serde_json::from_str(&raw).ok()
}

pub fn save_json<T: Serialize>(key: &str, value: &T) -> Result<(), Error> {
    let raw = serde_json::to_string(value)
        .map_err(|e| Error::Failed(format!("JSON serialize failed: {e}")))?;
    storage::set(key, &raw)
}

pub fn make_cache_key(
    engine: &str,
    source_lang: &str,
    target_lang: &str,
    mode: &str,
    query: &str,
) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        engine.trim().to_ascii_lowercase(),
        source_lang.trim().to_ascii_lowercase(),
        target_lang.trim().to_ascii_lowercase(),
        mode.trim().to_ascii_lowercase(),
        query.trim()
    )
}

pub fn load_cache() -> Vec<CacheEntry> {
    load_json(KEY_CACHE).unwrap_or_default()
}

pub fn prune_expired_cache(entries: &mut Vec<CacheEntry>, ttl_hours: u64) -> bool {
    if ttl_hours == 0 {
        return false;
    }
    let ttl_ms = ttl_hours.saturating_mul(3_600_000);
    let now = unix_millis_now();
    let before = entries.len();
    entries.retain(|e| now.saturating_sub(e.last_used_ms) <= ttl_ms);
    entries.len() != before
}

pub fn lookup_cache(
    entries: &mut [CacheEntry],
    engine: &str,
    source_lang: &str,
    target_lang: &str,
    query: &str,
) -> Option<TranslationResult> {
    lookup_cache_with_ttl(entries, engine, source_lang, target_lang, "text", query, 0)
}

pub fn lookup_cache_with_ttl(
    entries: &mut [CacheEntry],
    engine: &str,
    source_lang: &str,
    target_lang: &str,
    mode: &str,
    query: &str,
    ttl_hours: u64,
) -> Option<TranslationResult> {
    let key = make_cache_key(engine, source_lang, target_lang, mode, query);
    let now = unix_millis_now();
    let ttl_ms = ttl_hours.saturating_mul(3_600_000);
    if let Some(entry) = entries.iter_mut().find(|e| e.key == key) {
        if ttl_hours > 0 && now.saturating_sub(entry.last_used_ms) > ttl_ms {
            return None;
        }
        entry.last_used_ms = now;
        let mut r = entry.result.clone();
        r.from_cache = true;
        r.latency_ms = 0;
        return Some(r);
    }
    None
}

pub fn lookup_cache_by_term<'a>(
    entries: &'a [CacheEntry],
    raw_word: &str,
    split_word: Option<&str>,
) -> Option<&'a TranslationResult> {
    let lower_raw = raw_word.trim().to_ascii_lowercase();
    let lower_split = split_word.map(|s| s.trim().to_ascii_lowercase());
    entries
        .iter()
        .filter(|e| {
            e.lower_original == lower_raw
                || e.lower_query == lower_raw
                || lower_split
                    .as_ref()
                    .is_some_and(|sp| e.lower_original == *sp || e.lower_query == *sp)
        })
        .max_by_key(|e| {
            let text_bonus = if e.key.contains("|doc|") { 0 } else { 1 };
            (text_bonus, e.last_used_ms)
        })
        .map(|e| &e.result)
}

pub fn store_cache_entry(
    entries: &mut Vec<CacheEntry>,
    engine: &str,
    source_pref: &str,
    target_lang: &str,
    result: &TranslationResult,
) {
    store_cache_entry_with_mode(entries, engine, source_pref, target_lang, "text", result);
}

pub fn store_cache_entry_with_mode(
    entries: &mut Vec<CacheEntry>,
    engine: &str,
    source_pref: &str,
    target_lang: &str,
    mode: &str,
    result: &TranslationResult,
) {
    let key = make_cache_key(engine, source_pref, target_lang, mode, &result.query_text);
    entries.retain(|e| e.key != key);
    entries.insert(
        0,
        CacheEntry {
            key,
            lower_original: result.original_text.trim().to_ascii_lowercase(),
            lower_query: result.query_text.trim().to_ascii_lowercase(),
            result: result.clone(),
            last_used_ms: unix_millis_now(),
        },
    );
    if entries.len() > MAX_CACHE_ENTRIES {
        entries.truncate(MAX_CACHE_ENTRIES);
    }
    let _ = save_json(KEY_CACHE, entries);
}

pub fn clear_cache(entries: &mut Vec<CacheEntry>) -> Result<(), Error> {
    entries.clear();
    storage::delete(KEY_CACHE)
}

pub fn load_wordbook() -> Vec<WordBookEntry> {
    load_json(KEY_WORDBOOK).unwrap_or_default()
}

pub fn save_wordbook(entries: &[WordBookEntry]) -> Result<(), Error> {
    save_json(KEY_WORDBOOK, &entries)
}

pub fn find_wordbook<'a>(
    entries: &'a [WordBookEntry],
    raw_word: &str,
    split_word: Option<&str>,
) -> Option<&'a WordBookEntry> {
    let lower_raw = raw_word.trim().to_ascii_lowercase();
    let lower_split = split_word.map(|s| s.trim().to_ascii_lowercase());
    entries.iter().find(|e| {
        let ew = e.word.trim().to_ascii_lowercase();
        ew == lower_raw || lower_split.as_ref().is_some_and(|sp| ew == *sp)
    })
}

pub fn upsert_wordbook_entry(
    entry: WordBookEntry,
) -> Result<(Vec<WordBookEntry>, WordBookEntry, bool), Error> {
    let mut list = load_wordbook();
    let lower_word = entry.word.trim().to_ascii_lowercase();
    if let Some(pos) = list
        .iter()
        .position(|e| e.word.trim().to_ascii_lowercase() == lower_word)
    {
        let existing = &mut list[pos];
        existing.translation = entry.translation.clone();
        existing.source_lang = entry.source_lang.clone();
        existing.target_lang = entry.target_lang.clone();
        if !entry.phonetic.is_empty() {
            existing.phonetic = entry.phonetic.clone();
        }
        if !entry.explains.is_empty() {
            existing.explains = entry.explains.clone();
        }
        if !entry.tags.is_empty() {
            existing.tags = entry.tags.clone();
        }
        if !entry.note.is_empty() {
            existing.note = entry.note.clone();
        }
        if !entry.source_file.is_empty() {
            existing.source_file = entry.source_file.clone();
        }
        let updated = existing.clone();
        save_wordbook(&list)?;
        Ok((list, updated, false))
    } else {
        let mut created = entry;
        if created.id.is_empty() {
            created.id = format!("wb-{}", unix_millis_now());
        }
        list.insert(0, created.clone());
        save_wordbook(&list)?;
        Ok((list, created, true))
    }
}

pub fn rename_wordbook_tag(
    old_tag: &str,
    new_tag: &str,
) -> Result<(Vec<WordBookEntry>, usize), Error> {
    let old_trimmed = old_tag.trim();
    let new_trimmed = new_tag.trim();
    if old_trimmed.is_empty() || new_trimmed.is_empty() {
        return Err(Error::Invalid("Tag name cannot be empty".into()));
    }
    let mut list = load_wordbook();
    let mut changed = 0usize;
    for entry in &mut list {
        if entry.tags.iter().any(|t| t == old_trimmed) {
            let mut next = Vec::with_capacity(entry.tags.len());
            for t in &entry.tags {
                let candidate = if t == old_trimmed { new_trimmed } else { t.as_str() };
                if !next.iter().any(|x: &String| x == candidate) {
                    next.push(candidate.to_owned());
                }
            }
            entry.tags = next;
            changed += 1;
        }
    }
    if changed > 0 {
        save_wordbook(&list)?;
    }
    Ok((list, changed))
}

pub fn delete_wordbook_tag(tag: &str) -> Result<(Vec<WordBookEntry>, usize), Error> {
    let target = tag.trim();
    if target.is_empty() {
        return Err(Error::Invalid("Tag name cannot be empty".into()));
    }
    let mut list = load_wordbook();
    let mut changed = 0usize;
    for entry in &mut list {
        let before = entry.tags.len();
        entry.tags.retain(|t| t != target);
        if entry.tags.len() != before {
            changed += 1;
        }
    }
    if changed > 0 {
        save_wordbook(&list)?;
    }
    Ok((list, changed))
}

pub fn pick_word_of_the_day(entries: &[WordBookEntry]) -> Option<&WordBookEntry> {
    if entries.is_empty() {
        return None;
    }
    let unmastered: Vec<&WordBookEntry> = entries.iter().filter(|e| !e.mastered).collect();
    let pool: Vec<&WordBookEntry> = if unmastered.is_empty() {
        entries.iter().collect()
    } else {
        unmastered
    };
    let day_seed = (unix_millis_now() / 86_400_000) as usize;
    let idx = day_seed % pool.len();
    Some(pool[idx])
}

pub fn load_history() -> Vec<HistoryItem> {
    load_json(KEY_HISTORY).unwrap_or_default()
}

pub fn record_history(result: &TranslationResult, max_items: usize) -> Vec<HistoryItem> {
    let mut list = load_history();
    let norm_orig = result.original_text.trim().to_owned();
    let norm_trans = result.translation.trim().to_owned();
    list.retain(|h| {
        !(h.original_text == norm_orig
            && h.target_lang == result.target_lang
            && h.engine == result.engine)
    });
    let now = unix_millis_now();
    list.insert(
        0,
        HistoryItem {
            id: format!("hist-{now}"),
            engine: result.engine.clone(),
            original_text: norm_orig,
            query_text: result.query_text.clone(),
            translation: norm_trans,
            source_lang: result.source_lang.clone(),
            target_lang: result.target_lang.clone(),
            timestamp_ms: now,
        },
    );
    if list.len() > max_items {
        list.truncate(max_items);
    }
    let _ = save_json(KEY_HISTORY, &list);
    list
}

fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

fn cdata_safe(input: &str) -> String {
    input.replace("]]>", "]]]]><![CDATA[>")
}

fn xml_escape_text(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn export_wordbook(entries: &[WordBookEntry], format: &str) -> Result<(String, String), Error> {
    match format {
        "json" => {
            let text = serde_json::to_string_pretty(entries)
                .map_err(|e| Error::Failed(format!("WordBook JSON error: {e}")))?;
            Ok(("wordbook.json".into(), text))
        }
        "csv" => {
            let mut out = String::from("word,phonetic,translation,explains,tags,note,sourceLang,targetLang,mastered\n");
            for e in entries {
                let row = [
                    csv_escape(&e.word),
                    csv_escape(&e.phonetic),
                    csv_escape(&e.translation),
                    csv_escape(&e.explains.join("; ")),
                    csv_escape(&e.tags.join(",")),
                    csv_escape(&e.note),
                    csv_escape(&e.source_lang),
                    csv_escape(&e.target_lang),
                    e.mastered.to_string(),
                ]
                .join(",");
                out.push_str(&row);
                out.push('\n');
            }
            Ok(("wordbook.csv".into(), out))
        }
        "tsv" => {
            let mut out = String::new();
            for e in entries {
                let back = if e.explains.is_empty() {
                    e.translation.clone()
                } else {
                    format!("{} — {}", e.translation, e.explains.join("; "))
                };
                let front = if e.phonetic.is_empty() {
                    e.word.clone()
                } else {
                    format!("{} [{}]", e.word, e.phonetic)
                };
                out.push_str(&format!(
                    "{}\t{}\t{}\n",
                    front.replace(['\t', '\n'], " "),
                    back.replace(['\t', '\n'], " "),
                    e.tags.join(" ")
                ));
            }
            Ok(("wordbook-anki.tsv".into(), out))
        }
        "markdown" | "md" => {
            let mut out = String::from("| Word | Phonetic | Translation | Tags | Note |\n| --- | --- | --- | --- | --- |\n");
            for e in entries {
                let esc = |s: &str| s.replace('|', "\\|").replace('\n', " ");
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    esc(&e.word),
                    esc(&e.phonetic),
                    esc(&e.translation),
                    esc(&e.tags.join(", ")),
                    esc(&e.note)
                ));
            }
            Ok(("wordbook.md".into(), out))
        }
        "xml" => {
            let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<wordbook>\n");
            for e in entries {
                out.push_str(&format!(
                    "  <entry>\n    <word>{}</word>\n    <phonetic>{}</phonetic>\n    <translation>{}</translation>\n    <explains>{}</explains>\n    <tags>{}</tags>\n    <note>{}</note>\n    <sourceLang>{}</sourceLang>\n    <targetLang>{}</targetLang>\n    <mastered>{}</mastered>\n  </entry>\n",
                    xml_escape_text(&e.word),
                    xml_escape_text(&e.phonetic),
                    xml_escape_text(&e.translation),
                    xml_escape_text(&e.explains.join("; ")),
                    xml_escape_text(&e.tags.join(",")),
                    xml_escape_text(&e.note),
                    xml_escape_text(&e.source_lang),
                    xml_escape_text(&e.target_lang),
                    e.mastered,
                ));
            }
            out.push_str("</wordbook>\n");
            Ok(("wordbook.xml".into(), out))
        }
        "youdao-xml" => {
            let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<wordbook>\n");
            for e in entries {
                let trans = if e.explains.is_empty() {
                    e.translation.clone()
                } else {
                    format!("{}\n{}", e.translation, e.explains.join("\n"))
                };
                out.push_str(&format!(
                    "  <item>\n    <word><![CDATA[{}]]></word>\n    <trans><![CDATA[{}]]></trans>\n    <phonetic><![CDATA[{}]]></phonetic>\n    <tags><![CDATA[{}]]></tags>\n    <progress>{}</progress>\n  </item>\n",
                    cdata_safe(&e.word),
                    cdata_safe(&trans),
                    cdata_safe(&e.phonetic),
                    cdata_safe(&e.tags.join(",")),
                    if e.mastered { 1 } else { 0 },
                ));
            }
            out.push_str("</wordbook>\n");
            Ok(("wordbook-youdao.xml".into(), out))
        }
        "txt" => {
            let mut out = String::new();
            for e in entries {
                let tags_part = if e.tags.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", e.tags.join(", "))
                };
                out.push_str(&format!("{} - {}{}\n", e.word, e.translation, tags_part));
            }
            Ok(("wordbook.txt".into(), out))
        }
        other => Err(Error::Invalid(format!("Unsupported export format: {other}"))),
    }
}

fn extract_xml_tag(block: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(s) = block.find(&open).map(|i| i + open.len()) else {
        return String::new();
    };
    let Some(e_rel) = block[s..].find(&close) else {
        return String::new();
    };
    let mut inner = block[s..s + e_rel].trim().to_owned();
    if inner.starts_with("<![CDATA[") && inner.ends_with("]]>") {
        inner = inner[9..inner.len() - 3].to_owned();
    }
    inner
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .trim()
        .to_owned()
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' && cur.is_empty() {
            in_quotes = true;
        } else if c == ',' {
            fields.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    fields.push(cur);
    fields
}

/// Imports Word Book entries from JSON, XML / Youdao XML, CSV, TSV, or plain TXT text,
/// skipping duplicates by lowercase word.
pub fn import_wordbook_text(raw: &str) -> Result<(Vec<WordBookEntry>, usize), Error> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Error::Invalid("Import payload is empty".into()));
    }
    let mut existing = load_wordbook();
    let mut seen: HashSet<String> = existing
        .iter()
        .map(|e| e.word.trim().to_ascii_lowercase())
        .collect();
    let mut added = 0usize;
    let now = unix_millis_now();

    if trimmed.starts_with('[') {
        let imported: Vec<WordBookEntry> = serde_json::from_str(trimmed)
            .map_err(|e| Error::Invalid(format!("Invalid WordBook JSON: {e}")))?;
        for mut item in imported {
            let key = item.word.trim().to_ascii_lowercase();
            if key.is_empty() || !seen.insert(key) {
                continue;
            }
            if item.id.is_empty() {
                item.id = format!("wb-imp-{now}-{added}");
            }
            if item.created_at_ms == 0 {
                item.created_at_ms = now;
            }
            existing.push(item);
            added += 1;
        }
    } else if trimmed.starts_with('<') {
        let re = Regex::new(r"(?s)<(?:item|entry)>(.*?)</(?:item|entry)>")
            .map_err(|e| Error::Failed(format!("XML parser init error: {e}")))?;
        for cap in re.captures_iter(trimmed) {
            let block = &cap[1];
            let word = extract_xml_tag(block, "word");
            if word.is_empty() || !seen.insert(word.to_ascii_lowercase()) {
                continue;
            }
            let phonetic = extract_xml_tag(block, "phonetic");
            let trans = {
                let t = extract_xml_tag(block, "translation");
                if t.is_empty() { extract_xml_tag(block, "trans") } else { t }
            };
            let mut trans_lines = trans.lines();
            let primary_trans = trans_lines.next().unwrap_or("").trim().to_owned();
            let explains: Vec<String> = {
                let explicit = extract_xml_tag(block, "explains");
                if !explicit.is_empty() {
                    explicit
                        .split(';')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                } else {
                    trans_lines
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                }
            };
            let tags: Vec<String> = extract_xml_tag(block, "tags")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            let note = extract_xml_tag(block, "note");
            let source_lang = {
                let s = extract_xml_tag(block, "sourceLang");
                if s.is_empty() { "auto".to_owned() } else { s }
            };
            let target_lang = {
                let t = extract_xml_tag(block, "targetLang");
                if t.is_empty() { "zh-CN".to_owned() } else { t }
            };
            let mastered = extract_xml_tag(block, "mastered") == "true"
                || extract_xml_tag(block, "progress") == "1";

            existing.push(WordBookEntry {
                id: format!("wb-imp-{now}-{added}"),
                word,
                source_lang,
                target_lang,
                translation: primary_trans,
                phonetic,
                explains,
                tags,
                note,
                source_file: String::new(),
                created_at_ms: now,
                review_count: 0,
                mastered,
            });
            added += 1;
        }
    } else {
        for (idx, line) in trimmed.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if !line.contains('\t') && !line.contains(',') {
                // Plain TXT: `word - translation [tag1, tag2]` or bare `word`
                let (word_part, rest_trans) = line
                    .split_once(" - ")
                    .map(|(w, t)| (w.trim(), t.trim()))
                    .unwrap_or((line, ""));
                if word_part.is_empty() || !seen.insert(word_part.to_ascii_lowercase()) {
                    continue;
                }
                existing.push(WordBookEntry {
                    id: format!("wb-imp-{now}-{added}"),
                    word: word_part.to_owned(),
                    source_lang: "auto".into(),
                    target_lang: "zh-CN".into(),
                    translation: rest_trans.to_owned(),
                    phonetic: String::new(),
                    explains: Vec::new(),
                    tags: Vec::new(),
                    note: String::new(),
                    source_file: String::new(),
                    created_at_ms: now,
                    review_count: 0,
                    mastered: false,
                });
                added += 1;
                continue;
            }

            let cols: Vec<String> = if line.contains('\t') {
                line.split('\t').map(|s| s.trim().to_owned()).collect()
            } else {
                parse_csv_line(line)
            };
            if idx == 0
                && cols.first().is_some_and(|c| c.eq_ignore_ascii_case("word"))
                && cols.get(2).is_some_and(|c| c.eq_ignore_ascii_case("translation"))
            {
                continue;
            }
            let word = cols.first().map(|s| s.trim()).unwrap_or("");
            if word.is_empty() || !seen.insert(word.to_ascii_lowercase()) {
                continue;
            }
            let (phonetic, translation, explains, tags, note, source_lang, target_lang, mastered) =
                if cols.len() >= 3 && !line.contains('\t') {
                    (
                        cols.get(1).cloned().unwrap_or_default(),
                        cols.get(2).cloned().unwrap_or_default(),
                        cols.get(3)
                            .map(|s| {
                                s.split(';')
                                    .map(str::trim)
                                    .filter(|x| !x.is_empty())
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default(),
                        cols.get(4)
                            .map(|s| {
                                s.split(',')
                                    .map(str::trim)
                                    .filter(|x| !x.is_empty())
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default(),
                        cols.get(5).cloned().unwrap_or_default(),
                        cols.get(6).cloned().unwrap_or_else(|| "en".into()),
                        cols.get(7).cloned().unwrap_or_else(|| "zh-CN".into()),
                        cols.get(8).is_some_and(|s| s == "true"),
                    )
                } else {
                    (
                        String::new(),
                        cols.get(1).cloned().unwrap_or_default(),
                        Vec::new(),
                        cols.get(2)
                            .map(|s| {
                                s.split([' ', ','])
                                    .map(str::trim)
                                    .filter(|x| !x.is_empty())
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default(),
                        String::new(),
                        "auto".into(),
                        "zh-CN".into(),
                        false,
                    )
                };
            existing.push(WordBookEntry {
                id: format!("wb-imp-{now}-{added}"),
                word: word.to_owned(),
                source_lang,
                target_lang,
                translation,
                phonetic,
                explains,
                tags,
                note,
                source_file: String::new(),
                created_at_ms: now,
                review_count: 0,
                mastered,
            });
            added += 1;
        }
    }

    save_wordbook(&existing)?;
    Ok((existing, added))
}

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::mock::MockHost;

    #[test]
    fn cache_lru_ttl_and_term_lookup_work() {
        let _host = MockHost::install().with_capabilities(&["storage"]);
        let mut cache = Vec::new();
        let res = TranslationResult {
            engine: "google".into(),
            original_text: "getUserProfile".into(),
            query_text: "get User Profile".into(),
            translation: "获取用户资料".into(),
            source_lang: "en".into(),
            target_lang: "zh-CN".into(),
            ..TranslationResult::default()
        };
        store_cache_entry(&mut cache, "google", "auto", "zh-CN", &res);

        let hit = lookup_cache(&mut cache, "google", "auto", "zh-CN", "get User Profile").unwrap();
        assert!(hit.from_cache);
        assert_eq!(hit.translation, "获取用户资料");

        let by_raw = lookup_cache_by_term(&cache, "getUserProfile", None).unwrap();
        assert_eq!(by_raw.translation, "获取用户资料");

        cache[0].last_used_ms = 1;
        assert!(lookup_cache_with_ttl(&mut cache, "google", "auto", "zh-CN", "text", "get User Profile", 1).is_none());
    }

    #[test]
    fn wordbook_upsert_tags_and_all_export_import_roundtrips() {
        let _host = MockHost::install().with_capabilities(&["storage"]);
        let entry = WordBookEntry {
            id: String::new(),
            word: "idempotent".into(),
            source_lang: "en".into(),
            target_lang: "zh-CN".into(),
            translation: "幂等的".into(),
            phonetic: "/ˌaɪdɛmˈpoʊtənt/".into(),
            explains: vec!["adj. 幂等的".into()],
            tags: vec!["backend".into(), "http".into()],
            note: "Safe to retry".into(),
            source_file: "api.rs".into(),
            created_at_ms: 1000,
            review_count: 0,
            mastered: false,
        };
        let (list, _, created) = upsert_wordbook_entry(entry).unwrap();
        assert!(created);
        assert_eq!(list.len(), 1);
        assert!(pick_word_of_the_day(&list).is_some());

        let (renamed, r_count) = rename_wordbook_tag("backend", "server").unwrap();
        assert_eq!(r_count, 1);
        assert!(renamed[0].tags.contains(&"server".to_owned()));

        let (_, csv) = export_wordbook(&renamed, "csv").unwrap();
        let (_, tsv) = export_wordbook(&renamed, "tsv").unwrap();
        let (_, md) = export_wordbook(&renamed, "markdown").unwrap();
        let (_, xml) = export_wordbook(&renamed, "xml").unwrap();
        let (_, yd_xml) = export_wordbook(&renamed, "youdao-xml").unwrap();
        let (_, txt) = export_wordbook(&renamed, "txt").unwrap();
        assert!(tsv.contains("idempotent [/ˌaɪdɛmˈpoʊtənt/]\t幂等的"));
        assert!(md.contains("| idempotent |"));
        assert!(xml.contains("<word>idempotent</word>"));
        assert!(yd_xml.contains("<![CDATA[idempotent]]>"));
        assert!(txt.contains("idempotent - 幂等的"));

        for payload in [csv, xml, yd_xml] {
            save_wordbook(&[]).unwrap();
            let (reimported, count) = import_wordbook_text(&payload).unwrap();
            assert_eq!(count, 1);
            assert_eq!(reimported[0].word, "idempotent");
            assert_eq!(reimported[0].translation, "幂等的");
        }

        let (after_del, d_count) = delete_wordbook_tag("server").unwrap();
        assert_eq!(d_count, 1);
        assert!(!after_del[0].tags.contains(&"server".to_owned()));
    }
}
