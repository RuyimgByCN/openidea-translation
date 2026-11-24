use openidea_plugin::text::{offset, position};
use openidea_plugin::{Position, Range};
use regex::Regex;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedTarget {
    pub raw_text: String,
    pub query_text: String,
    pub range: Range,
    pub was_split: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceCandidate {
    pub style: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentBlock {
    pub start_line: u32,
    pub end_line: u32,
    pub range: Range,
    pub raw_text: String,
    pub clean_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocChunk {
    pub text: String,
    pub translatable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectedText {
    pub text: String,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct IpynbCellSource {
    pub cell_index: usize,
    pub is_array: bool,
    pub source: String,
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3040..=0x30FF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF
    )
}

fn is_separator_char(ch: char, extra_separators: &str) -> bool {
    matches!(ch, '_' | '-' | '.' | ':' | '/' | '$') || extra_separators.contains(ch)
}

fn is_ident_char(ch: char, extra_separators: &str) -> bool {
    ch.is_ascii_alphanumeric() || is_separator_char(ch, extra_separators)
}

fn is_word_or_cjk(ch: char, extra_separators: &str) -> bool {
    is_ident_char(ch, extra_separators)
        || is_cjk_char(ch)
        || (ch.is_alphabetic() && !ch.is_whitespace())
}

/// Converts a single line/UTF-16 position to a byte offset in `text` (clamped safely).
fn safe_offset(text: &str, pos: Position) -> Option<usize> {
    offset(text, pos)
}

/// Applies an optional ignore regular expression before translating text.
/// Invalid patterns safely fall back to returning `text` unchanged.
pub fn apply_ignore_regexp(text: &str, pattern: &str) -> String {
    let trimmed_pattern = pattern.trim();
    if trimmed_pattern.is_empty() || text.is_empty() {
        return text.to_owned();
    }
    let Ok(re) = Regex::new(trimmed_pattern) else {
        return text.to_owned();
    };
    let replaced = re.replace_all(text, "");
    let cleaned_lines: Vec<&str> = replaced
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if cleaned_lines.is_empty() {
        String::new()
    } else if text.contains('\n') {
        cleaned_lines.join("\n")
    } else {
        cleaned_lines.join(" ")
    }
}

/// Extracts the active selection or auto-selects a word/identifier/CJK run under the caret.
pub fn extract_editor_target(
    text: &str,
    selection: Range,
    mode: &str,
    auto_split: bool,
) -> Option<ExtractedTarget> {
    extract_editor_target_with_options(text, selection, mode, auto_split, "_-.", "")
}

/// Configurable version of `extract_editor_target` supporting custom separator characters and `ignoreRegExp`.
pub fn extract_editor_target_with_options(
    text: &str,
    selection: Range,
    mode: &str,
    auto_split: bool,
    separators: &str,
    ignore_regexp: &str,
) -> Option<ExtractedTarget> {
    if !selection.is_empty() {
        let a = safe_offset(text, selection.start)?;
        let b = safe_offset(text, selection.end)?;
        let (start_b, end_b) = (a.min(b), a.max(b));
        let raw_slice = text.get(start_b..end_b)?;
        let raw = raw_slice.trim().to_owned();
        if raw.is_empty() {
            return None;
        }
        let cleaned = apply_ignore_regexp(&raw, ignore_regexp);
        if cleaned.trim().is_empty() {
            return None;
        }
        let range = Range::new(position(text, start_b), position(text, end_b));
        let split = if auto_split {
            split_identifier_with_separators(&cleaned, separators)
        } else {
            None
        };
        let (query_text, was_split) = match split {
            Some(s) => (s, true),
            None => (cleaned, false),
        };
        return Some(ExtractedTarget {
            raw_text: raw,
            query_text,
            range,
            was_split,
        });
    }

    let caret_off = safe_offset(text, selection.start)?;
    let (start_b, end_b) = if mode.eq_ignore_ascii_case("exclusive") {
        exclusive_bounds_at(text, caret_off, separators)?
    } else {
        inclusive_bounds_at(text, caret_off, separators)?
    };
    let raw = text.get(start_b..end_b)?.to_owned();
    if raw.trim().is_empty() {
        return None;
    }
    let cleaned = apply_ignore_regexp(&raw, ignore_regexp);
    if cleaned.trim().is_empty() {
        return None;
    }
    let range = Range::new(position(text, start_b), position(text, end_b));
    let split = if auto_split {
        split_identifier_with_separators(&cleaned, separators)
    } else {
        None
    };
    let (query_text, was_split) = match split {
        Some(s) => (s, true),
        None => (cleaned, false),
    };
    Some(ExtractedTarget {
        raw_text: raw,
        query_text,
        range,
        was_split,
    })
}

fn anchor_char_pos(text: &str, caret_off: usize, separators: &str) -> Option<(usize, char)> {
    if let Some((idx, ch)) = text[caret_off..].char_indices().next()
        && is_word_or_cjk(ch, separators)
    {
        return Some((caret_off + idx, ch));
    }
    if caret_off > 0
        && let Some((idx, ch)) = text[..caret_off].char_indices().next_back()
        && is_word_or_cjk(ch, separators)
    {
        return Some((idx, ch));
    }
    None
}

fn inclusive_bounds_at(text: &str, caret_off: usize, separators: &str) -> Option<(usize, usize)> {
    let (anchor, ch) = anchor_char_pos(text, caret_off, separators)?;
    if is_cjk_char(ch) {
        let mut start = anchor;
        for (idx, c) in text[..anchor].char_indices().rev() {
            if is_cjk_char(c) {
                start = idx;
            } else {
                break;
            }
        }
        let mut end = anchor + ch.len_utf8();
        for (idx, c) in text[end..].char_indices() {
            if is_cjk_char(c) {
                end = anchor + ch.len_utf8() + idx + c.len_utf8();
            } else {
                break;
            }
        }
        return Some((start, end));
    }

    let mut start = anchor;
    for (idx, c) in text[..anchor].char_indices().rev() {
        if is_ident_char(c, separators) || (c.is_alphabetic() && !is_cjk_char(c)) {
            start = idx;
        } else {
            break;
        }
    }
    let mut end = anchor + ch.len_utf8();
    let tail_base = end;
    for (idx, c) in text[tail_base..].char_indices() {
        if is_ident_char(c, separators) || (c.is_alphabetic() && !is_cjk_char(c)) {
            end = tail_base + idx + c.len_utf8();
        } else {
            break;
        }
    }
    Some((start, end))
}

fn exclusive_bounds_at(text: &str, caret_off: usize, separators: &str) -> Option<(usize, usize)> {
    let (full_start, full_end) = inclusive_bounds_at(text, caret_off, separators)?;
    let full = &text[full_start..full_end];
    if full.chars().any(is_cjk_char) {
        return Some((full_start, full_end));
    }
    let rel_caret = caret_off.clamp(full_start, full_end) - full_start;
    let spans = camel_snake_spans(full, separators);
    if spans.is_empty() {
        return Some((full_start, full_end));
    }
    for (s, e) in &spans {
        if rel_caret >= *s && rel_caret < *e {
            return Some((full_start + s, full_start + e));
        }
    }
    for (s, e) in &spans {
        if rel_caret == *e {
            return Some((full_start + s, full_start + e));
        }
    }
    let first = spans[0];
    Some((full_start + first.0, full_start + first.1))
}

/// Returns byte spans of sub-words inside an identifier token.
fn camel_snake_spans(token: &str, extra_separators: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let chars: Vec<(usize, char)> = token.char_indices().collect();
    if chars.is_empty() {
        return spans;
    }

    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && is_separator_char(chars[i].1, extra_separators) {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let start_byte = chars[i].0;
        let mut j = i + 1;
        while j < chars.len() {
            let prev = chars[j - 1].1;
            let curr = chars[j].1;
            if is_separator_char(curr, extra_separators) {
                break;
            }
            let next = chars.get(j + 1).map(|x| x.1);
            let camel_boundary = prev.is_lowercase() && curr.is_uppercase();
            let acronym_boundary =
                prev.is_uppercase() && curr.is_uppercase() && next.is_some_and(char::is_lowercase);
            let digit_boundary = (prev.is_ascii_alphabetic() && curr.is_ascii_digit())
                || (prev.is_ascii_digit() && curr.is_ascii_alphabetic());
            if camel_boundary || acronym_boundary || digit_boundary {
                break;
            }
            j += 1;
        }
        let end_byte = if j < chars.len() {
            chars[j].0
        } else {
            token.len()
        };
        if end_byte > start_byte {
            spans.push((start_byte, end_byte));
        }
        i = j;
    }
    spans
}

/// Splits code identifiers using default boundaries (`_-.:/$`).
pub fn split_identifier(input: &str) -> Option<String> {
    split_identifier_with_separators(input, "_-.")
}

/// Splits code identifiers (`camelCase`, `PascalCase`, `XMLHttpRequest`,
/// `snake_case`, `kebab-case`, `dot.case`, `foo::bar`, or custom `separators`)
/// into natural space-separated words. Returns `None` if `input` is already plain prose or a single simple word.
pub fn split_identifier_with_separators(input: &str, separators: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }
    if trimmed.contains(' ') {
        return None;
    }
    if trimmed.chars().any(is_cjk_char) {
        return None;
    }
    let words: Vec<String> = camel_snake_spans(trimmed, separators)
        .into_iter()
        .map(|(s, e)| trimmed[s..e].to_owned())
        .collect();
    if words.len() <= 1 {
        return None;
    }
    let joined = words.join(" ");
    if joined == trimmed {
        None
    } else {
        Some(joined)
    }
}

/// Separates surrounding whitespace and matching outer quote delimiters (`"`, `'`, `` ` ``)
/// so `keepFormat` can preserve them across replacement.
pub fn preserve_format_wrapper(original_selected: &str) -> (&str, &str, &str) {
    let leading_ws_len = original_selected
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map_or(original_selected.len(), |(i, _)| i);
    if leading_ws_len == original_selected.len() {
        return (original_selected, "", "");
    }
    let trailing_ws_start = original_selected
        .char_indices()
        .rfind(|(_, c)| !c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let middle = &original_selected[leading_ws_len..trailing_ws_start];

    for quote in ['"', '\'', '`'] {
        if middle.len() >= 2
            && middle.starts_with(quote)
            && middle.ends_with(quote)
            && middle[1..middle.len() - 1].chars().any(|c| !c.is_whitespace())
        {
            return (
                &original_selected[..leading_ws_len + 1],
                &middle[1..middle.len() - 1],
                &original_selected[trailing_ws_start - 1..],
            );
        }
    }

    (
        &original_selected[..leading_ws_len],
        middle,
        &original_selected[trailing_ws_start..],
    )
}

/// Extracts identifier-safe ASCII words from a translated English phrase.
fn ascii_words(phrase: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    for ch in phrase.chars() {
        if ch.is_ascii_alphanumeric() {
            cur.push(ch);
        } else if !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn title_word(word: &str) -> String {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::new();
    out.push(first.to_ascii_uppercase());
    for c in chars {
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Default `generate_replace_candidates` preserving previous call signatures.
pub fn generate_replace_candidates(
    primary: &str,
    alternatives: &[String],
    preferred_style: &str,
) -> Vec<ReplaceCandidate> {
    generate_replace_candidates_ext(primary, alternatives, preferred_style, true, "")
}

/// Extended candidate generator supporting article stripping, `dot.case`,
/// `Title Case`, `UPPER CASE`, `lower case`, and custom separator output.
pub fn generate_replace_candidates_ext(
    primary: &str,
    alternatives: &[String],
    preferred_style: &str,
    strip_articles: bool,
    custom_separators: &str,
) -> Vec<ReplaceCandidate> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    let push_unique = |out: &mut Vec<ReplaceCandidate>,
                       seen: &mut HashSet<String>,
                       style: &str,
                       text: String| {
        let clean = text.trim().to_owned();
        if !clean.is_empty() && seen.insert(clean.clone()) {
            out.push(ReplaceCandidate {
                style: style.to_owned(),
                text: clean,
            });
        }
    };

    let raw_words = ascii_words(primary);
    let ident_words: Vec<String> = if strip_articles && raw_words.len() > 1 {
        let filtered: Vec<String> = raw_words
            .iter()
            .filter(|w| !matches!(w.to_ascii_lowercase().as_str(), "a" | "an" | "the"))
            .cloned()
            .collect();
        if filtered.is_empty() {
            raw_words.clone()
        } else {
            filtered
        }
    } else {
        raw_words.clone()
    };

    let can_format_ident = !ident_words.is_empty() && ident_words.len() <= 8 && primary.is_ascii();

    if can_format_ident {
        let camel = {
            let mut s = ident_words[0].to_ascii_lowercase();
            for w in &ident_words[1..] {
                s.push_str(&title_word(w));
            }
            s
        };
        let pascal: String = ident_words.iter().map(|w| title_word(w)).collect();
        let snake: String = ident_words
            .iter()
            .map(|w| w.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("_");
        let screaming: String = ident_words
            .iter()
            .map(|w| w.to_ascii_uppercase())
            .collect::<Vec<_>>()
            .join("_");
        let kebab: String = ident_words
            .iter()
            .map(|w| w.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("-");
        let dot: String = ident_words
            .iter()
            .map(|w| w.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(".");
        let title_case: String = raw_words
            .iter()
            .map(|w| title_word(w))
            .collect::<Vec<_>>()
            .join(" ");
        let upper_case: String = raw_words
            .iter()
            .map(|w| w.to_ascii_uppercase())
            .collect::<Vec<_>>()
            .join(" ");
        let lower_case: String = raw_words
            .iter()
            .map(|w| w.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ");

        let mut all_styles = vec![
            ("camelCase", camel),
            ("PascalCase", pascal),
            ("snake_case", snake),
            ("SCREAMING_SNAKE_CASE", screaming),
            ("kebab-case", kebab),
            ("dot.case", dot),
            ("Title Case", title_case),
            ("UPPER CASE", upper_case),
            ("lower case", lower_case),
            ("original", primary.trim().to_owned()),
        ];

        for sep in custom_separators.chars() {
            if !matches!(sep, '_' | '-' | '.' | ' ') && !sep.is_whitespace() && ident_words.len() > 1
            {
                let mut sep_buf = [0u8; 4];
                let sep_str = sep.encode_utf8(&mut sep_buf);
                let joined = ident_words
                    .iter()
                    .map(|w| w.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join(sep_str);
                all_styles.push(("custom", joined));
                break;
            }
        }

        for (style, text) in &all_styles {
            if *style == preferred_style {
                push_unique(&mut out, &mut seen, style, text.clone());
            }
        }
        for (style, text) in all_styles {
            push_unique(&mut out, &mut seen, style, text);
        }
    } else {
        push_unique(&mut out, &mut seen, "original", primary.trim().to_owned());
    }

    for alt in alternatives.iter().take(6) {
        push_unique(&mut out, &mut seen, "alternative", alt.clone());
    }

    out
}

/// Replaces Javadoc/KDoc/JSDoc inline tags (`{@link Foo}`, `@param id`), HTML inline tags
/// (`<code>...</code>`, `<a ...>...</a>`, `<br/>`), Markdown inline code `` `...` ``, and URLs
/// with indexed placeholders (`[[TT_0]]`, ...) so translation engines preserve them verbatim.
pub fn protect_inline_tags(text: &str) -> ProtectedText {
    let Ok(re) = Regex::new(
        r"(\{@[A-Za-z][A-Za-z0-9_-]*(?:\s+[^}]*)?\}|</?(?:code|kbd|var|pre|a|strong|em|b|i|br)(?:\s+[^>]*)?/?>|`[^`\n]+`|https?://[^\s)\]>]+|(?:^|\s)@(?:param|return|returns|throws|exception|see|since|deprecated|author|version|suppress|property|constructor|receiver|sample)\b(?:\s+[A-Za-z0-9_.$<>]+)?)",
    ) else {
        return ProtectedText {
            text: text.to_owned(),
            tags: Vec::new(),
        };
    };

    let mut tags = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;

    for m in re.find_iter(text) {
        let full = m.as_str();
        // Preserve leading whitespace before @param style block tags cleanly
        let lead_ws = full.len() - full.trim_start().len();
        out.push_str(&text[last..m.start() + lead_ws]);
        let tag_str = &full[lead_ws..];
        let idx = tags.len();
        tags.push(tag_str.to_owned());
        out.push_str(&format!("[[TT_{idx}]]"));
        last = m.end();
    }
    out.push_str(&text[last..]);

    ProtectedText { text: out, tags }
}

/// Restores `[[TT_n]]` placeholders produced by `protect_inline_tags`, tolerating optional
/// spaces injected inside placeholder brackets by neural/machine translators.
pub fn restore_inline_tags(translated: &str, tags: &[String]) -> String {
    if tags.is_empty() {
        return translated.to_owned();
    }
    let Ok(re) = Regex::new(r"\[\[\s*TT_(\d+)\s*\]\]") else {
        return translated.to_owned();
    };
    re.replace_all(translated, |caps: &regex::Captures| {
        caps.get(1)
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .and_then(|idx| tags.get(idx))
            .map(String::as_str)
            .unwrap_or(caps.get(0).map_or("", |m| m.as_str()))
            .to_owned()
    })
    .into_owned()
}

fn line_byte_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for (idx, b) in text.bytes().enumerate() {
        if b == b'\n' {
            let end = if idx > start && text.as_bytes()[idx - 1] == b'\r' {
                idx - 1
            } else {
                idx
            };
            ranges.push((start, end));
            start = idx + 1;
        }
    }
    if start <= text.len() {
        ranges.push((start, text.len()));
    }
    ranges
}

fn strip_comment_decorators(raw: &str) -> String {
    let mut lines = Vec::new();
    for line in raw.lines() {
        let mut t = line.trim();
        if let Some(rest) = t.strip_prefix("/**") {
            t = rest.trim_start();
        } else if let Some(rest) = t.strip_prefix("/*!") {
            t = rest.trim_start();
        } else if let Some(rest) = t.strip_prefix("/*") {
            t = rest.trim_start();
        }
        if let Some(rest) = t.strip_suffix("*/") {
            t = rest.trim_end();
        }
        if let Some(rest) = t.strip_prefix("<!--") {
            t = rest.trim_start();
        }
        if let Some(rest) = t.strip_suffix("-->") {
            t = rest.trim_end();
        }
        if let Some(rest) = t.strip_prefix("\"\"\"") {
            t = rest.trim_start();
        }
        if let Some(rest) = t.strip_suffix("\"\"\"") {
            t = rest.trim_end();
        }
        if let Some(rest) = t.strip_prefix("'''") {
            t = rest.trim_start();
        }
        if let Some(rest) = t.strip_suffix("'''") {
            t = rest.trim_end();
        }
        t = t
            .strip_prefix("///")
            .or_else(|| t.strip_prefix("//!"))
            .or_else(|| t.strip_prefix("//"))
            .or_else(|| t.strip_prefix("##"))
            .or_else(|| t.strip_prefix('#'))
            .or_else(|| t.strip_prefix("--"))
            .or_else(|| t.strip_prefix('*'))
            .unwrap_or(t)
            .trim();
        lines.push(t);
    }

    let mut out = Vec::new();
    let mut prev_blank = true;
    for l in lines {
        if l.is_empty() {
            if !prev_blank {
                out.push("");
                prev_blank = true;
            }
        } else {
            out.push(l);
            prev_blank = false;
        }
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

fn line_prefix_kind(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("///") {
        Some("///")
    } else if trimmed.starts_with("//!") {
        Some("//!")
    } else if trimmed.starts_with("//") {
        Some("//")
    } else if trimmed.starts_with('#') && !trimmed.starts_with("#[") && !trimmed.starts_with("#!") {
        Some("#")
    } else if trimmed.starts_with("--") {
        Some("--")
    } else {
        None
    }
}

/// Finds the contiguous comment / doc comment block surrounding `pos`.
pub fn extract_comment_block_at(text: &str, pos: Position) -> Option<CommentBlock> {
    let lines = line_byte_ranges(text);
    if lines.is_empty() {
        return None;
    }
    let target_line = (pos.line as usize).min(lines.len().saturating_sub(1));

    let caret_off = safe_offset(text, pos).unwrap_or(lines[target_line].0);
    for (open, close) in [("/*", "*/"), ("<!--", "-->"), ("\"\"\"", "\"\"\""), ("'''", "'''")] {
        let mut search_from = 0usize;
        while let Some(rel_open) = text[search_from..].find(open) {
            let abs_open = search_from + rel_open;
            let after_open = abs_open + open.len();
            if let Some(rel_close) = text[after_open..].find(close) {
                let abs_close = after_open + rel_close + close.len();
                let line_start = lines[target_line].0;
                let line_end = lines[target_line].1;
                if abs_open <= line_end && abs_close >= line_start && abs_open <= caret_off.max(line_end)
                {
                    let raw = text[abs_open..abs_close].to_owned();
                    let clean = strip_comment_decorators(&raw);
                    if !clean.is_empty() {
                        let start_pos = position(text, abs_open);
                        let end_pos = position(text, abs_close);
                        return Some(CommentBlock {
                            start_line: start_pos.line,
                            end_line: end_pos.line,
                            range: Range::new(start_pos, end_pos),
                            raw_text: raw,
                            clean_text: clean,
                        });
                    }
                }
                search_from = abs_close;
            } else {
                break;
            }
        }
    }

    let cur_trimmed = text[lines[target_line].0..lines[target_line].1].trim();
    if let Some(kind) = line_prefix_kind(cur_trimmed) {
        let mut first = target_line;
        while first > 0 {
            let prev = text[lines[first - 1].0..lines[first - 1].1].trim();
            if line_prefix_kind(prev) == Some(kind) {
                first -= 1;
            } else {
                break;
            }
        }
        let mut last = target_line;
        while last + 1 < lines.len() {
            let next = text[lines[last + 1].0..lines[last + 1].1].trim();
            if line_prefix_kind(next) == Some(kind) {
                last += 1;
            } else {
                break;
            }
        }
        let start_b = lines[first].0;
        let end_b = lines[last].1;
        let raw = text[start_b..end_b].to_owned();
        let clean = strip_comment_decorators(&raw);
        if !clean.is_empty() {
            return Some(CommentBlock {
                start_line: first as u32,
                end_line: last as u32,
                range: Range::new(position(text, start_b), position(text, end_b)),
                raw_text: raw,
                clean_text: clean,
            });
        }
    }

    let line_slice = &text[lines[target_line].0..lines[target_line].1];
    for marker in ["///", "//", "# ", "-- "] {
        if let Some(idx) = line_slice.find(marker) {
            let abs_start = lines[target_line].0 + idx;
            let abs_end = lines[target_line].1;
            let raw = text[abs_start..abs_end].to_owned();
            let clean = strip_comment_decorators(&raw);
            if !clean.is_empty() {
                return Some(CommentBlock {
                    start_line: target_line as u32,
                    end_line: target_line as u32,
                    range: Range::new(position(text, abs_start), position(text, abs_end)),
                    raw_text: raw,
                    clean_text: clean,
                });
            }
        }
    }

    None
}

/// Extracts all comment blocks across a file for inline annotations or batch comment translation.
pub fn extract_all_comments(text: &str, max_blocks: usize) -> Vec<CommentBlock> {
    let lines = line_byte_ranges(text);
    let mut blocks = Vec::new();
    let mut idx = 0usize;

    while idx < lines.len() && blocks.len() < max_blocks {
        let line = &text[lines[idx].0..lines[idx].1];
        let trimmed = line.trim();

        if trimmed.starts_with("/*") {
            let start_line = idx;
            let start_b = lines[idx].0 + line.find("/*").unwrap_or(0);
            let mut end_line = idx;
            let mut end_b = lines[idx].1;
            if let Some(close_pos) = line[line.find("/*").unwrap_or(0) + 2..].find("*/") {
                end_b = start_b + 2 + close_pos + 2;
            } else {
                while end_line + 1 < lines.len() {
                    end_line += 1;
                    let next_line = &text[lines[end_line].0..lines[end_line].1];
                    if let Some(c) = next_line.find("*/") {
                        end_b = lines[end_line].0 + c + 2;
                        break;
                    }
                    end_b = lines[end_line].1;
                }
            }
            let raw = text[start_b..end_b].to_owned();
            let clean = strip_comment_decorators(&raw);
            if clean.chars().any(|c| c.is_alphanumeric() || is_cjk_char(c)) {
                blocks.push(CommentBlock {
                    start_line: start_line as u32,
                    end_line: end_line as u32,
                    range: Range::new(position(text, start_b), position(text, end_b)),
                    raw_text: raw,
                    clean_text: clean,
                });
            }
            idx = end_line + 1;
            continue;
        }

        if let Some(kind) = line_prefix_kind(trimmed) {
            let start_line = idx;
            while idx + 1 < lines.len() {
                let next_trimmed = text[lines[idx + 1].0..lines[idx + 1].1].trim();
                if line_prefix_kind(next_trimmed) == Some(kind) {
                    idx += 1;
                } else {
                    break;
                }
            }
            let start_b = lines[start_line].0;
            let end_b = lines[idx].1;
            let raw = text[start_b..end_b].to_owned();
            let clean = strip_comment_decorators(&raw);
            if clean.chars().any(|c| c.is_alphanumeric() || is_cjk_char(c)) {
                blocks.push(CommentBlock {
                    start_line: start_line as u32,
                    end_line: idx as u32,
                    range: Range::new(position(text, start_b), position(text, end_b)),
                    raw_text: raw,
                    clean_text: clean,
                });
            }
            idx += 1;
            continue;
        }

        idx += 1;
    }

    blocks
}

/// Splits a document into chunks while leaving Markdown fenced code blocks untouched (`translatable: false`).
pub fn chunk_document_for_translation(
    text: &str,
    _language: &str,
    max_chars: usize,
) -> Vec<DocChunk> {
    let mut chunks = Vec::new();
    let mut buf = String::new();
    let mut in_fence = false;
    let mut fence_marker = "";

    for segment in text.split_inclusive('\n') {
        let trimmed = segment.trim_start();
        let starts_backtick = trimmed.starts_with("```");
        let starts_tilde = trimmed.starts_with("~~~");

        if !in_fence && (starts_backtick || starts_tilde) {
            if !buf.is_empty() {
                push_split_chunks(&mut chunks, std::mem::take(&mut buf), true, max_chars);
            }
            in_fence = true;
            fence_marker = if starts_backtick { "```" } else { "~~~" };
            buf.push_str(segment);
            continue;
        }

        if in_fence {
            buf.push_str(segment);
            if trimmed.starts_with(fence_marker) {
                chunks.push(DocChunk {
                    text: std::mem::take(&mut buf),
                    translatable: false,
                });
                in_fence = false;
                fence_marker = "";
            }
            continue;
        }

        buf.push_str(segment);
        if buf.chars().count() >= max_chars && segment.trim().is_empty() {
            push_split_chunks(&mut chunks, std::mem::take(&mut buf), true, max_chars);
        }
    }

    if !buf.is_empty() {
        if in_fence {
            chunks.push(DocChunk {
                text: buf,
                translatable: false,
            });
        } else {
            push_split_chunks(&mut chunks, buf, true, max_chars);
        }
    }

    chunks
}

fn push_split_chunks(
    out: &mut Vec<DocChunk>,
    text: String,
    translatable: bool,
    max_chars: usize,
) {
    if text.chars().count() <= max_chars || !translatable {
        out.push(DocChunk { text, translatable });
        return;
    }
    let mut cur = String::new();
    for line in text.split_inclusive('\n') {
        if !cur.is_empty() && cur.chars().count() + line.chars().count() > max_chars {
            out.push(DocChunk {
                text: std::mem::take(&mut cur),
                translatable: true,
            });
        }
        cur.push_str(line);
    }
    if !cur.is_empty() {
        out.push(DocChunk {
            text: cur,
            translatable: true,
        });
    }
}

/// Extracts Markdown & raw cells from a Jupyter `.ipynb` JSON document, leaving code cells intact.
pub fn extract_ipynb_markdown_cells(raw_json: &str) -> Result<Vec<IpynbCellSource>, String> {
    let root: serde_json::Value =
        serde_json::from_str(raw_json).map_err(|e| format!("Invalid .ipynb JSON: {e}"))?;
    let cells = root
        .get("cells")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Missing `cells` array in .ipynb notebook".to_owned())?;

    let mut out = Vec::new();
    for (idx, cell) in cells.iter().enumerate() {
        let cell_type = cell
            .get("cell_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !matches!(cell_type, "markdown" | "raw") {
            continue;
        }
        let Some(src) = cell.get("source") else {
            continue;
        };
        let (is_array, joined) = if let Some(arr) = src.as_array() {
            let s: String = arr.iter().filter_map(|v| v.as_str()).collect();
            (true, s)
        } else if let Some(s) = src.as_str() {
            (false, s.to_owned())
        } else {
            continue;
        };
        if !joined.trim().is_empty() {
            out.push(IpynbCellSource {
                cell_index: idx,
                is_array,
                source: joined,
            });
        }
    }
    Ok(out)
}

/// Applies translated Markdown/raw cell contents back into a `.ipynb` JSON document cleanly.
pub fn apply_ipynb_translations(
    raw_json: &str,
    replacements: &[(usize, String)],
    bilingual: bool,
    fold_original: bool,
) -> Result<String, String> {
    let mut root: serde_json::Value =
        serde_json::from_str(raw_json).map_err(|e| format!("Invalid .ipynb JSON: {e}"))?;
    let Some(cells) = root.get_mut("cells").and_then(|v| v.as_array_mut()) else {
        return Err("Missing `cells` array in .ipynb notebook".to_owned());
    };

    for (cell_index, translated) in replacements {
        let Some(cell) = cells.get_mut(*cell_index) else {
            continue;
        };
        let Some(src) = cell.get_mut("source") else {
            continue;
        };
        let (is_array, orig) = if let Some(arr) = src.as_array() {
            let s: String = arr.iter().filter_map(|v| v.as_str()).collect();
            (true, s)
        } else if let Some(s) = src.as_str() {
            (false, s.to_owned())
        } else {
            continue;
        };

        let next_text = if bilingual {
            if fold_original {
                format!(
                    "<details>\n<summary>Original</summary>\n\n{}\n\n</details>\n\n{}",
                    orig.trim(),
                    translated.trim()
                )
            } else {
                format!("{}\n\n---\n\n{}", orig.trim(), translated.trim())
            }
        } else {
            translated.clone()
        };

        if is_array {
            let lines: Vec<serde_json::Value> = if next_text.is_empty() {
                Vec::new()
            } else {
                next_text
                    .split_inclusive('\n')
                    .map(|l| serde_json::Value::String(l.to_owned()))
                    .collect()
            };
            *src = serde_json::Value::Array(lines);
        } else {
            *src = serde_json::Value::String(next_text);
        }
    }

    serde_json::to_string_pretty(&root).map_err(|e| format!("Failed to serialize .ipynb: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_identifiers_intelligently_with_custom_separators() {
        assert_eq!(split_identifier("getUserProfile").as_deref(), Some("get User Profile"));
        assert_eq!(
            split_identifier("XMLHttpRequest2Parser").as_deref(),
            Some("XML Http Request 2 Parser")
        );
        assert_eq!(split_identifier("user_auth_token").as_deref(), Some("user auth token"));
        assert_eq!(split_identifier("kebab-case-name").as_deref(), Some("kebab case name"));
        assert_eq!(
            split_identifier_with_separators("user@auth#token", "@#").as_deref(),
            Some("user auth token")
        );
        assert_eq!(split_identifier("hello world"), None);
        assert_eq!(split_identifier("hello"), None);
    }

    #[test]
    fn ignore_regexp_strips_configured_prefixes() {
        let cleaned = apply_ignore_regexp("/// 用户认证服务", r"^///\s*");
        assert_eq!(cleaned, "用户认证服务");
        // Invalid regex degrades safely
        assert_eq!(apply_ignore_regexp("keep me", r"[unclosed"), "keep me");
    }

    #[test]
    fn keep_format_preserves_whitespace_and_quotes() {
        let (p, core, s) = preserve_format_wrapper("  \"用户名称\" ");
        assert_eq!(p, "  \"");
        assert_eq!(core, "用户名称");
        assert_eq!(s, "\" ");

        let (p2, core2, s2) = preserve_format_wrapper("\tgetUser\n");
        assert_eq!(p2, "\t");
        assert_eq!(core2, "getUser");
        assert_eq!(s2, "\n");
    }

    #[test]
    fn generates_naming_style_candidates_with_articles_and_extra_cases() {
        let cands = generate_replace_candidates(
            "User authentication service",
            &["Auth Service".into()],
            "snake_case",
        );
        assert_eq!(cands[0].style, "snake_case");
        assert_eq!(cands[0].text, "user_authentication_service");
        assert!(cands.iter().any(|c| c.style == "camelCase" && c.text == "userAuthenticationService"));
        assert!(cands.iter().any(|c| c.style == "PascalCase" && c.text == "UserAuthenticationService"));
        assert!(cands.iter().any(|c| c.style == "SCREAMING_SNAKE_CASE" && c.text == "USER_AUTHENTICATION_SERVICE"));
        assert!(cands.iter().any(|c| c.style == "kebab-case" && c.text == "user-authentication-service"));
        assert!(cands.iter().any(|c| c.style == "dot.case" && c.text == "user.authentication.service"));
        assert!(cands.iter().any(|c| c.style == "Title Case" && c.text == "User Authentication Service"));
        assert!(cands.iter().any(|c| c.style == "UPPER CASE" && c.text == "USER AUTHENTICATION SERVICE"));
        assert!(cands.iter().any(|c| c.style == "lower case" && c.text == "user authentication service"));
        assert!(cands.iter().any(|c| c.style == "alternative" && c.text == "Auth Service"));

        let stripped = generate_replace_candidates_ext("create a user profile", &[], "camelCase", true, "");
        assert_eq!(stripped[0].text, "createUserProfile");
    }

    #[test]
    fn protects_and_restores_inline_tags() {
        let raw = "Fetches {@link UserService} using `auth_token` from https://example.com/api.\n@param id user identifier";
        let prot = protect_inline_tags(raw);
        assert!(prot.text.contains("[[TT_0]]"));
        assert!(prot.text.contains("[[TT_1]]"));
        assert!(prot.text.contains("[[TT_2]]"));
        assert!(prot.text.contains("[[TT_3]]"));
        let simulated_translation = prot.text.replace("[[TT_0]]", "[[ TT_0 ]]");
        let restored = restore_inline_tags(&simulated_translation, &prot.tags);
        assert_eq!(restored, raw);
    }

    #[test]
    fn ipynb_translates_markdown_cells_only() {
        let nb = r##"{
  "cells": [
    { "cell_type": "markdown", "source": ["# Hello\n", "World"] },
    { "cell_type": "code", "source": ["print('hello')"] }
  ],
  "nbformat": 4
}"##;
        let md_cells = extract_ipynb_markdown_cells(nb).unwrap();
        assert_eq!(md_cells.len(), 1);
        assert_eq!(md_cells[0].cell_index, 0);
        let updated = apply_ipynb_translations(nb, &[(0, "# 你好\n世界".into())], false, false).unwrap();
        assert!(updated.contains("print('hello')"));
        assert!(updated.contains("# 你好"));
    }

    #[test]
    fn inclusive_and_exclusive_selection_with_utf16_offsets() {
        let text = "let a = \"你好\"; let userProfileName = 1;";
        let profile_pos = position(text, text.find("Profile").unwrap() + 2);
        let sel = Range::new(profile_pos, profile_pos);

        let inc = extract_editor_target(text, sel, "inclusive", true).unwrap();
        assert_eq!(inc.raw_text, "userProfileName");
        assert_eq!(inc.query_text, "user Profile Name");
        assert!(inc.was_split);

        let exc = extract_editor_target(text, sel, "exclusive", true).unwrap();
        assert_eq!(exc.raw_text, "Profile");
        assert_eq!(exc.query_text, "Profile");
        assert!(!exc.was_split);

        let cjk_pos = position(text, text.find("你好").unwrap());
        let cjk_sel = Range::new(cjk_pos, cjk_pos);
        let cjk = extract_editor_target(text, cjk_sel, "inclusive", true).unwrap();
        assert_eq!(cjk.raw_text, "你好");
    }

    #[test]
    fn extracts_doc_and_line_comment_blocks() {
        let src = "/**\n * Fetches user profile.\n * @param id user id\n */\nfn get() {}\n// line one\n// line two\nlet a = 1;\n";
        let block = extract_comment_block_at(src, Position::new(1, 5)).unwrap();
        assert_eq!(block.clean_text, "Fetches user profile.\n@param id user id");
        assert_eq!(block.start_line, 0);
        assert_eq!(block.end_line, 3);

        let line_block = extract_comment_block_at(src, Position::new(5, 3)).unwrap();
        assert_eq!(line_block.clean_text, "line one\nline two");

        let all = extract_all_comments(src, 50);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn preserves_fenced_code_blocks_in_document_chunks() {
        let md = "# Title\n\nIntro paragraph.\n\n```rust\nfn main() {}\n```\n\nOutro.";
        let chunks = chunk_document_for_translation(md, "markdown", 2000);
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].translatable);
        assert!(!chunks[1].translatable);
        assert!(chunks[1].text.contains("fn main() {}"));
        assert!(chunks[2].translatable);
    }
}
