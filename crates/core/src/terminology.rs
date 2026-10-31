//! Local spelling context: explicit aliases and unambiguous near-spellings of long names.
use crate::{library::Entry, Engine, UserNote};
use anyhow::Result;
use regex::{Regex, RegexBuilder};
use rusqlite::OptionalExtension;
use std::collections::{BTreeMap, BTreeSet, HashSet};
mod document;
pub(crate) use document::NoteSpellingEdit;

#[cfg(test)]
thread_local! {
    // A one-shot, thread-local boundary lets tests coordinate concurrent saves
    // without timing assumptions or a production runtime mode.
    static BEFORE_CORRECTION_LOCK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

pub struct Terms {
    matcher: Option<Regex>,
    names: BTreeMap<String, String>,
    ambiguous: BTreeSet<String>,
    ambiguous_matcher: Option<Regex>,
    ambiguous_compacts: HashSet<String>,
    model_aliases: HashSet<String>,
    pub hotwords: Vec<String>,
    recognition_aliases: Vec<(String, String)>,
    priorities: BTreeMap<String, u8>,
    writer_names: Vec<String>,
    caption_names: Vec<String>,
    actor_spellings: Vec<String>,
    approximate: Vec<String>,
}

fn decoder_spelling(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= crate::asr_hotwords::MAX_WORD_CHARS
        && value.chars().all(|c| c.is_alphanumeric() || c == ' ')
}

fn display_spelling(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= crate::asr_hotwords::MAX_WORD_CHARS
        && !value.chars().any(char::is_control)
}

fn identifier_spelling(value: &str) -> bool {
    value.chars().skip(1).any(|c| c.is_ascii_uppercase())
        || value.chars().any(|c| c.is_ascii_digit())
        || value.contains(['.', '+', '_'])
}

fn prioritize_spellings(words: &mut Vec<String>, priorities: &BTreeMap<String, u8>) {
    words.sort_by(|a, b| priorities[a].cmp(&priorities[b]).then(a.cmp(b)));
    let mut seen = HashSet::new();
    words.retain(|word| seen.insert(word.to_lowercase()));
    words.truncate(crate::asr_hotwords::MAX_WORDS);
}

// A complete explicit graphics-series keyword may supply a model number.
// Monetary abbreviations such as "RMB 3000" are not graphics identifiers.
pub(crate) fn model_identifier(value: &str) -> Option<(&str, &str)> {
    static MODEL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let parts = MODEL
        .get_or_init(|| Regex::new(r"(?i)^([a-z]{2,8})[ \t]*([0-9]{2,8})$").unwrap())
        .captures(value)?;
    let prefix = parts.get(1)?.as_str();
    if !["gtx", "rtx", "gts", "gt", "mx", "rx", "arc"]
        .iter()
        .any(|series| prefix.eq_ignore_ascii_case(series))
    {
        return None;
    }
    Some((prefix, parts.get(2)?.as_str()))
}

fn spoken_digits(digits: &str, zero: char) -> String {
    digits
        .chars()
        .map(|c| match c {
            '0' => zero,
            '1' => '一',
            '2' => '二',
            '3' => '三',
            '4' => '四',
            '5' => '五',
            '6' => '六',
            '7' => '七',
            '8' => '八',
            '9' => '九',
            _ => unreachable!("model_identifier permits only ASCII digits"),
        })
        .collect()
}

fn english_digits(digits: &str) -> String {
    digits
        .chars()
        .map(|c| match c {
            '0' => "ZERO",
            '1' => "ONE",
            '2' => "TWO",
            '3' => "THREE",
            '4' => "FOUR",
            '5' => "FIVE",
            '6' => "SIX",
            '7' => "SEVEN",
            '8' => "EIGHT",
            '9' => "NINE",
            _ => unreachable!("model_identifier permits only ASCII digits"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn model_spellings(name: &str) -> Option<Vec<String>> {
    let (prefix, digits) = model_identifier(name)?;
    let mut variants = vec![
        name.to_owned(),
        format!("{prefix}{digits}"),
        format!("{prefix} {digits}"),
    ];
    for zero in ['零', '〇'] {
        let spoken = spoken_digits(digits, zero);
        let separated = spoken
            .chars()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        for number in [spoken, separated] {
            variants.push(format!("{prefix}{number}"));
            variants.push(format!("{prefix} {number}"));
        }
    }
    variants.push(format!("{prefix} {}", english_digits(digits)));
    Some(variants)
}

pub(crate) const MODEL_CONTINUATION_UNITS: &[&str] = &[
    "美元",
    "人民币",
    "公斤",
    "千克",
    "毫秒",
    "毫升",
    "元",
    "个",
    "件",
    "号",
    "张",
    "台",
    "年",
    "月",
    "日",
    "次",
    "份",
    "块",
    "分",
    "支",
    "款",
    "吨",
    "米",
    "秒",
    "%",
    "％",
];

fn model_continuation(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || c.is_numeric()
        || c == '_'
        || "零〇一二三四五六七八九十百千万亿两幺点%％".contains(c)
}

pub(crate) fn model_has_continuation(text: &str) -> bool {
    let trimmed = text.trim_start_matches([' ', '\t']);
    if (trimmed.starts_with('.') || trimmed.starts_with('．') || trimmed.starts_with('点'))
        && trimmed[trimmed.chars().next().unwrap().len_utf8()..]
            .trim_start_matches([' ', '\t'])
            .chars()
            .next()
            .is_some_and(|c| c.is_numeric() || "零〇一二三四五六七八九".contains(c))
    {
        return true;
    }
    if MODEL_CONTINUATION_UNITS
        .iter()
        .any(|unit| trimmed.starts_with(unit))
    {
        return true;
    }
    if text.chars().next().is_some_and(model_continuation) {
        return true;
    }
    let rest = text.trim_start_matches([' ', '\t']);
    if rest.len() == text.len() {
        return false;
    }
    let mut chars = rest.chars();
    let first = chars.next();
    if first.is_some_and(|c| c.is_numeric() || "元个张台年月日次份块".contains(c)) {
        return true;
    }
    if first.is_some_and(|c| "零〇一二三四五六七八九十百千万亿两幺".contains(c))
        && chars.next().is_none_or(|c| {
            c.is_ascii_alphanumeric()
                || c.is_numeric()
                || c.is_ascii_whitespace()
                || c.is_ascii_punctuation()
                || "零〇一二三四五六七八九十百千万亿两幺元个张台年月日次份块".contains(c)
        })
    {
        return true;
    }
    let word = rest
        .split(|c: char| !c.is_ascii_alphanumeric())
        .next()
        .unwrap_or_default();
    [
        "ZERO", "ONE", "TWO", "THREE", "FOUR", "FIVE", "SIX", "SEVEN", "EIGHT", "NINE", "TI",
        "SUPER", "XT", "POINT", "PERCENT", "DOLLAR", "DOLLARS", "YUAN", "EURO", "EUROS", "USD",
        "RMB", "CNY", "EUR", "GBP",
    ]
    .iter()
    .any(|suffix| word.eq_ignore_ascii_case(suffix))
}

/// A marked/standalone name with a Chinese honorific is an opaque spelling,
/// not evidence of gender or an English identity. Do not mine arbitrary prose
/// for surnames, or turn generic teaching/qualification roles into names.
fn note_honorific_spellings(content: &str) -> Vec<&str> {
    static LABEL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static MARKED: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let label = LABEL.get_or_init(|| {
        Regex::new(r"^(?P<stem>[\p{Han}]{1,4})(?:老师|老師|讲师|講師|教授|博士)$").unwrap()
    });
    let marked = MARKED.get_or_init(|| Regex::new(r"\*\*(?P<label>[\p{Han}]{3,6})\*\*").unwrap());
    let mut names = Vec::new();
    for value in marked
        .captures_iter(content)
        .filter_map(|c| c.name("label").map(|m| m.as_str()))
        .chain(
            content
                .lines()
                .map(|line| line.trim().trim_matches(['*', '-', '#', ' '])),
        )
    {
        let Some(parts) = label.captures(value) else {
            continue;
        };
        let stem = &parts["stem"];
        if [
            "各位", "所有", "这位", "那位", "一位", "某位", "几位", "多位", "任何", "任课", "授课",
            "指导", "助理", "副", "资深", "高级", "男", "女", "学校", "大学", "学院", "数学",
            "物理", "化学", "历史", "文学", "教育", "医学", "哲学", "在读", "应届",
        ]
        .contains(&stem)
            || stem.chars().count() > 1 && stem.contains(['和', '或', '与', '及', '、'])
            || names.contains(&value)
        {
            continue;
        }
        names.push(value);
        if names.len() == 80 {
            break;
        }
    }
    names
}

struct NoteWord<'a> {
    word: &'a str,
    named: bool,
}

/// The same bounded note admission serves glossary selection and normalization.
fn note_words(note: &UserNote) -> Vec<NoteWord<'_>> {
    static LATIN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let latin = LATIN.get_or_init(|| Regex::new(r"[A-Za-z][A-Za-z0-9_+.-]{2,39}").unwrap());
    let chinese_context = note
        .content
        .chars()
        .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
    let mut accepted_words = HashSet::new();
    let mut words = Vec::new();
    let mut marked_spans = note_asr_marked_spans(&note.content).peekable();
    for word in latin.find_iter(&note.content) {
        // A complete marked Unicode/long token must not leave an ASCII prefix
        // as a separate correction name or recognition hint.
        while marked_spans
            .peek()
            .is_some_and(|span| span.end() <= word.start())
        {
            marked_spans.next();
        }
        if marked_spans.peek().is_some_and(|span| {
            span.start() <= word.start()
                && word.end() <= span.end()
                && (note.content[..word.start()]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric)
                    || note.content[word.end()..]
                        .chars()
                        .next()
                        .is_some_and(char::is_alphanumeric))
        }) {
            continue;
        }
        let line_start = note.content[..word.start()]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let line_end = note.content[word.end()..]
            .find('\n')
            .map_or(note.content.len(), |i| word.end() + i);
        let line = note.content[line_start..line_end]
            .trim()
            .trim_matches(['*', '-', '#', ' ']);
        if model_identifier(line).is_some() {
            continue;
        }
        let word = word.as_str().trim_end_matches(['.', '-']);
        if accepted_words.contains(word) {
            continue;
        }
        if accepted_words.len() >= 80 {
            break;
        }
        accepted_words.insert(word.to_owned());
        if ["nbsp", "amp", "http", "https"].contains(&word.to_lowercase().as_str()) {
            continue;
        }
        // Standalone keywords may provide lowercase spellings. Concept headings
        // remain ASR focus; strong identifiers and explicit terms remain names.
        let keyword_line = note
            .content
            .lines()
            .any(|line| line.trim().trim_matches(['*', '-', '#', ' ']) == word);
        let identifier = word.chars().skip(1).any(|c| c.is_ascii_uppercase())
            || word.chars().any(|c| c.is_ascii_digit())
            || word.eq_ignore_ascii_case("omascribe");
        let concept = note.kind != "term"
            && !identifier
            && ["date", "test", "budget"].contains(&word.to_ascii_lowercase().as_str());
        let named = identifier
            || !concept
                && (keyword_line
                    || word.starts_with(char::is_uppercase)
                        && note.content.contains(&format!("**{word}**")));
        if note.kind == "term" || chinese_context || named || concept && keyword_line {
            words.push(NoteWord { word, named });
        }
    }
    words
}

/// Explicit complete spans may bias recognition without becoming literal name
/// obligations or correction aliases. Preserve every character and token.
fn note_asr_marked_spans(content: &str) -> impl Iterator<Item = regex::Match<'_>> {
    static MARKED: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static PHRASE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static SHORT: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static SINGLE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    // Consume an entire marked run before checking its size. A rejected long
    // label's closing ** must not pair with the next valid label's opening **.
    // Notes already have a fixed input ceiling; this match borrows the text.
    let marked = MARKED.get_or_init(|| Regex::new(r"\*\*(?P<value>[^*\r\n]*)\*\*").unwrap());
    let phrase = PHRASE
        .get_or_init(|| Regex::new(r"^[\p{Latin}\p{Nd}]+(?: +[\p{Latin}\p{Nd}]+)+$").unwrap());
    let short = SHORT.get_or_init(|| Regex::new(r"^(?:\p{Latin}\p{Nd}|\p{Nd}\p{Latin})$").unwrap());
    let single = SINGLE.get_or_init(|| Regex::new(r"^[\p{Latin}\p{Nd}]+$").unwrap());
    marked
        .captures_iter(content)
        .filter_map(|capture| capture.name("value"))
        .filter(move |span| {
            let value = span.as_str();
            let count = value
                .chars()
                .take(crate::asr_hotwords::MAX_WORD_CHARS + 1)
                .count();
            (1..=crate::asr_hotwords::MAX_WORD_CHARS).contains(&count)
                && model_identifier(value).is_none()
                && (short.is_match(value)
                    || phrase.is_match(value) && value.chars().any(char::is_uppercase)
                    || count >= 3
                        && single.is_match(value)
                        && value.chars().any(|c| !c.is_ascii() && c.is_alphabetic()))
        })
}

fn note_asr_marked_spellings(content: &str) -> Vec<&str> {
    let mut seen = HashSet::new();
    note_asr_marked_spans(content)
        .map(|span| span.as_str())
        .filter(|value| seen.insert(value.to_lowercase()))
        .take(crate::asr_hotwords::MAX_WORDS)
        .collect()
}

impl Terms {
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut model_aliases: Vec<_> = self.model_aliases.iter().collect();
        model_aliases.sort_unstable();
        let payload = if !self.ambiguous.is_empty() {
            serde_json::to_vec(&(
                "terms-v8-ambiguous-spellings",
                &self.names,
                &self.approximate,
                &self.caption_names,
                &self.writer_names,
                &model_aliases,
                &self.ambiguous,
            ))
        } else if model_aliases.is_empty() {
            serde_json::to_vec(&(
                "terms-v5-writer-names",
                &self.names,
                &self.approximate,
                &self.caption_names,
                &self.writer_names,
            ))
        } else {
            serde_json::to_vec(&(
                "terms-v6-models-writer-names",
                &self.names,
                &self.approximate,
                &self.caption_names,
                &self.writer_names,
                &model_aliases,
            ))
        };
        format!("{:x}", Sha256::digest(payload.expect("term signature")))
    }
    pub fn new(notes: &[UserNote], entries: &[Entry]) -> Self {
        let mut names = BTreeMap::new();
        let mut model_aliases = HashSet::new();
        let mut conflicts = HashSet::new();
        let mut hotwords = Vec::new();
        let mut recognition_aliases = Vec::new();
        let mut writer_names = Vec::new();
        let mut declared_note_names = HashSet::new();
        let mut caption_names = Vec::new();
        let mut approximate = Vec::new();
        let note_spellings: Vec<_> = crate::notes::document_notes(notes)
            .into_iter()
            .map(|note| {
                let asr_spellings = note_asr_marked_spellings(&note.content);
                let words = note_words(note);
                (
                    note,
                    note_honorific_spellings(&note.content),
                    words,
                    asr_spellings,
                )
            })
            .collect();
        let mut active_spellings = HashSet::new();
        let mut active_asr_spellings = HashSet::new();
        for (note, honorifics, words, asr_spellings) in &note_spellings {
            for spelling in asr_spellings {
                active_spellings.insert(spelling.to_lowercase());
                active_asr_spellings.insert(spelling.to_lowercase());
            }
            active_spellings.extend(honorifics.iter().map(|name| name.to_lowercase()));
            active_spellings.extend(
                words
                    .iter()
                    .filter(|word| word.named || note.kind == "term")
                    .map(|word| word.word.to_lowercase()),
            );
            let mut models = HashSet::new();
            for line in note.content.lines() {
                let name = line.trim().trim_matches(['*', '-', '#', ' ']);
                if let Some((prefix, digits)) = model_identifier(name) {
                    let compact = format!("{prefix}{digits}").to_lowercase();
                    if models.len() == 80 && !models.contains(&compact) {
                        break;
                    }
                    models.insert(compact.clone());
                    active_spellings.insert(name.to_lowercase());
                    active_spellings.insert(compact);
                    active_spellings.insert(format!("{prefix} {digits}").to_lowercase());
                }
            }
        }
        let mut glossary_names = HashSet::new();
        let mut expanded_models = HashSet::new();
        let explicit: HashSet<String> = entries
            .iter()
            .filter(|e| e.kind == "term" && e.enabled && !e.deleted)
            .flat_map(|e| {
                std::iter::once(e.title.as_str())
                    .chain(e.aliases.split([',', '，', ';', '；', '\n', '|']))
            })
            .map(|s| s.trim().to_lowercase())
            .collect();
        let mut add = |alias: &str, name: &str| {
            let alias = alias.trim();
            let name = name.trim();
            if !(2..=80).contains(&alias.chars().count()) || name.is_empty() {
                return;
            }
            let key = alias.to_lowercase();
            if names.get(&key).is_some_and(|old| old != name) {
                conflicts.insert(key.clone());
            }
            names.insert(key, name.to_owned());
        };
        let eligible = |entry: &&Entry| entry.kind == "term" && entry.enabled && !entry.deleted;
        let active = |entry: &&Entry| {
            std::iter::once(entry.title.as_str())
                .chain(entry.aliases.split([',', '，', ';', '；', '\n', '|']))
                .any(|spelling| active_spellings.contains(&spelling.trim().to_lowercase()))
        };
        // Select before taking the existing entry budget. Two ordered passes
        // retain input order within ranks without building an all-entry map.
        for e in entries
            .iter()
            .filter(eligible)
            .filter(active)
            .chain(
                entries
                    .iter()
                    .filter(eligible)
                    .filter(|entry| !active(entry)),
            )
            .take(80)
        {
            glossary_names.insert(e.title.trim().to_owned());
            hotwords.push(e.title.trim().to_owned());
            writer_names.push(e.title.trim().to_owned());
            caption_names.push(e.title.trim().to_owned());
            approximate.push(e.title.trim().to_owned());
            add(&e.title, &e.title);
            for alias in std::iter::once(e.title.as_str())
                .chain(e.aliases.split([',', '，', ';', '；', '\n', '|']))
            {
                add(alias, &e.title);
                // Punctuation is part of the display spelling, but native
                // hotword syntax stays strict. Only the user's complete,
                // explicit aliases can supply another recognition spelling.
                if !decoder_spelling(e.title.trim())
                    && display_spelling(e.title.trim())
                    && decoder_spelling(alias.trim())
                {
                    recognition_aliases.push((alias.trim().to_owned(), e.title.trim().to_owned()));
                }
                if let Some((prefix, digits)) = model_identifier(alias.trim()) {
                    model_aliases.insert(alias.trim().to_lowercase());
                    let key = (
                        format!("{}{digits}", prefix.to_ascii_lowercase()),
                        e.title.to_lowercase(),
                    );
                    if expanded_models.contains(&key) || expanded_models.len() < 80 {
                        let is_new = expanded_models.insert(key);
                        if is_new {
                            let variants =
                                model_spellings(alias.trim()).expect("validated identifier");
                            for variant in variants {
                                if explicit.contains(&variant.to_lowercase())
                                    && !variant.eq_ignore_ascii_case(alias.trim())
                                {
                                    continue;
                                }
                                model_aliases.insert(variant.to_lowercase());
                                add(&variant, &e.title);
                            }
                        }
                    }
                }
            }
        }
        // User-written names supply spelling/casing, never facts or inferred aliases.
        for (n, honorifics, words, asr_spellings) in note_spellings {
            for name in honorifics {
                if !explicit.contains(&name.to_lowercase()) {
                    add(name, name);
                    hotwords.push(name.to_owned());
                    writer_names.push(name.to_owned());
                    caption_names.push(name.to_owned());
                }
            }
            for line in n.content.lines() {
                let name = line.trim().trim_matches(['*', '-', '#', ' ']);
                let Some((prefix, digits)) = model_identifier(name) else {
                    continue;
                };
                let compact = format!("{prefix}{digits}");
                let spaced = format!("{prefix} {digits}");
                active_spellings.insert(name.to_lowercase());
                active_spellings.insert(compact.to_lowercase());
                active_spellings.insert(spaced.to_lowercase());
                // Explicit glossary names and aliases always take precedence.
                if explicit.contains(&compact.to_lowercase())
                    || explicit.contains(&spaced.to_lowercase())
                {
                    continue;
                }
                let key = (
                    compact.to_lowercase(),
                    format!("note:{}", compact.to_lowercase()),
                );
                if expanded_models.contains(&key) || expanded_models.len() >= 80 {
                    continue;
                }
                expanded_models.insert(key);
                for alias in model_spellings(name).expect("validated model identifier") {
                    if explicit.contains(&alias.to_lowercase()) {
                        continue;
                    }
                    model_aliases.insert(alias.to_lowercase());
                    add(&alias, name);
                }
                hotwords.push(name.to_owned());
                writer_names.push(name.to_owned());
                caption_names.push(name.to_owned());
            }
            for NoteWord { word, named } in words {
                let name = if word.eq_ignore_ascii_case("omascribe") {
                    "OmaScribe"
                } else {
                    word
                };
                if !explicit.contains(&word.to_lowercase()) {
                    add(word, name);
                    hotwords.push(name.to_owned());
                    // Mixed-language notes also supply ordinary ASR words.
                    // Only names explicitly marked as such constrain minutes;
                    // ordinary words must remain free to translate.
                    if named || n.kind == "term" {
                        writer_names.push(name.to_owned());
                        if n.kind == "term" {
                            declared_note_names.insert(name.to_owned());
                        }
                    }
                    // A generic keyword such as Budget is still translatable.
                    // Strong product identifiers and explicitly stored terms
                    // supply caption spellings, never additional speech facts.
                    if name.chars().skip(1).any(|c| c.is_ascii_uppercase())
                        || name.chars().any(|c| c.is_ascii_digit())
                        || named && name.contains(['.', '+', '_'])
                        || n.kind == "term"
                    {
                        caption_names.push(name.to_owned());
                    }
                    if named || n.kind == "term" {
                        approximate.push(name.to_owned());
                    }
                }
            }
            for spelling in asr_spellings {
                // Existing glossary ownership (including disputed aliases)
                // stays authoritative. Never revive a conflicting corrector.
                if !explicit.contains(&spelling.to_lowercase()) {
                    hotwords.push(spelling.to_owned());
                }
            }
        }
        // Known compound spelling variants, only activated by the user's explicit product name.
        if hotwords.iter().any(|s| s.eq_ignore_ascii_case("omascribe")) {
            for alias in [
                "OMA SCRIBE",
                "OMA SCRIB",
                "奥马SCRIB",
                "奥马 SCRIB",
                "奥马斯",
                "奥马的SCRIB",
                "奥马（OMA）的SCRIB",
                "奥马 (OMA) 的 SCRIB",
            ] {
                if !explicit.contains(&alias.to_lowercase()) {
                    add(alias, "OmaScribe");
                }
            }
        }
        // Candidate budgets cannot give a selected spelling sole ownership of
        // an alias also claimed by an eligible entry outside that budget.
        for entry in entries.iter().filter(eligible) {
            for alias in std::iter::once(entry.title.as_str())
                .chain(entry.aliases.split([',', '，', ';', '；', '\n', '|']))
            {
                let key = alias.trim().to_lowercase();
                if names
                    .get(&key)
                    .is_some_and(|selected| selected != entry.title.trim())
                {
                    conflicts.insert(key);
                }
                if let Some((prefix, digits)) = model_identifier(alias.trim()) {
                    let compact = format!("{prefix}{digits}").to_lowercase();
                    let spaced = format!("{prefix} {digits}").to_lowercase();
                    let selected_owner = names.get(&compact).or_else(|| names.get(&spaced));
                    if (model_aliases.contains(&compact) || model_aliases.contains(&spaced))
                        && selected_owner.is_some_and(|owner| owner != entry.title.trim())
                    {
                        // Check only a model family already admitted into the
                        // selected map; an unselected claimant must also revoke
                        // its generated digit/spacing aliases, not just one key.
                        for variant in model_spellings(alias.trim()).unwrap() {
                            let key = variant.to_lowercase();
                            if model_aliases.contains(&key)
                                && (!explicit.contains(&key) || conflicts.contains(&key))
                                && names
                                    .get(&key)
                                    .is_some_and(|selected| selected != entry.title.trim())
                            {
                                conflicts.insert(key);
                            }
                        }
                    }
                }
            }
        }
        let ambiguous: BTreeSet<_> = conflicts.into_iter().collect();
        for key in &ambiguous {
            names.remove(key);
        }
        // The regex engine uses Unicode case folding, which is broader than
        // lowercase keys (for example long s matches S). Keep the alternative
        // identity in a separate matcher instead of guessing it from hit text.
        let ambiguous_matcher = (!ambiguous.is_empty()).then(|| {
            let mut aliases: Vec<_> = ambiguous.iter().collect();
            aliases.sort_by_key(|alias| std::cmp::Reverse(alias.len()));
            RegexBuilder::new(
                &aliases
                    .iter()
                    .map(|alias| regex::escape(alias))
                    .collect::<Vec<_>>()
                    .join("|"),
            )
            .case_insensitive(true)
            .build()
            .unwrap()
        });
        let ambiguous_compacts = ambiguous
            .iter()
            .filter(|alias| {
                alias
                    .chars()
                    .all(|c| c.is_ascii_alphabetic() || c == ' ' || c == '-')
            })
            .map(|alias| alias.chars().filter(char::is_ascii_alphabetic).collect())
            .collect();
        // A note may name an explicit glossary alias. Resolve priority through
        // the final, non-conflicting map without changing canonical spelling.
        let active_names: HashSet<_> = active_spellings
            .iter()
            .filter_map(|spelling| names.get(spelling))
            .collect();
        let priorities: BTreeMap<_, _> = hotwords
            .iter()
            .chain(&writer_names)
            .chain(&caption_names)
            .map(|name| {
                let priority = if active_names.contains(name)
                    || active_asr_spellings.contains(&name.to_lowercase())
                {
                    0
                } else if glossary_names.contains(name) {
                    1
                } else {
                    2
                };
                (name.clone(), priority)
            })
            .collect();
        model_aliases.retain(|key| names.contains_key(key));
        // Keep ambiguous literals in the matcher as opaque, unchanged spans:
        // shorter aliases inside them must not acquire replacement authority.
        let mut aliases: Vec<_> = names.keys().chain(&ambiguous).collect();
        aliases.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let matcher = (!aliases.is_empty()).then(|| {
            RegexBuilder::new(
                &aliases
                    .iter()
                    .map(|s| regex::escape(s))
                    .collect::<Vec<_>>()
                    .join("|"),
            )
            .case_insensitive(true)
            .build()
            .unwrap()
        });
        hotwords.retain(|s| decoder_spelling(s));
        prioritize_spellings(&mut hotwords, &priorities);
        writer_names.retain(|s| {
            display_spelling(s)
                && names.get(&s.to_lowercase()) == Some(s)
                && (decoder_spelling(s)
                    || identifier_spelling(s)
                    || glossary_names.contains(s)
                    || declared_note_names.contains(s))
        });
        prioritize_spellings(&mut writer_names, &priorities);
        caption_names.retain(|s| display_spelling(s) && names.get(&s.to_lowercase()) == Some(s));
        prioritize_spellings(&mut caption_names, &priorities);
        recognition_aliases.retain(|(alias, canonical)| {
            writer_names.contains(canonical) && names.get(&alias.to_lowercase()) == Some(canonical)
        });
        recognition_aliases.sort_by(|(alias, canonical), (other_alias, other_canonical)| {
            priorities[canonical]
                .cmp(&priorities[other_canonical])
                .then(canonical.cmp(other_canonical))
                .then(alias.cmp(other_alias))
        });
        approximate.retain(|s| {
            (8..=40).contains(&s.len())
                && s.chars().all(|c| c.is_ascii_alphabetic())
                && names.get(&s.to_lowercase()) == Some(s)
        });
        prioritize_spellings(&mut approximate, &priorities);
        Self {
            matcher,
            names,
            ambiguous,
            ambiguous_matcher,
            ambiguous_compacts,
            model_aliases,
            hotwords,
            recognition_aliases,
            priorities,
            writer_names,
            caption_names,
            actor_spellings: Vec::new(),
            approximate,
        }
    }
    /// Corroborate source spellings without inventing aliases or changing ASR
    /// hotwords. Callers supply current-run sources and explicitly known manual
    /// labels. An anchor is admitted only when its exact spelling occurs in
    /// those sources; a bare date clause never guesses a name boundary.
    ///
    /// This context does not affect the normalizer fingerprint. Translation and
    /// minutes callers must bind actor_spellings_digest to their own identities.
    pub(crate) fn with_actor_spellings<'a>(
        mut self,
        sources: impl IntoIterator<Item = &'a str>,
        anchors: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        fn add(names: &mut Vec<String>, value: &str) {
            if names.len() < 12
                && (2..=80).contains(&value.chars().count())
                && value
                    .chars()
                    .all(|c| c.is_alphanumeric() || " '-’".contains(c))
                && !names.iter().any(|s| s.eq_ignore_ascii_case(value))
                && names.iter().map(|s| s.chars().count()).sum::<usize>() + value.chars().count()
                    <= 240
            {
                names.push(value.to_owned());
            }
        }
        let mut manual = Vec::new();
        for label in anchors {
            add(&mut manual, label.trim());
        }
        let mut names = Vec::new();
        for source in sources {
            for label in &manual {
                if contains_name(source, label) {
                    add(&mut names, label);
                }
            }
            for actor in source_owner_names(source) {
                add(&mut names, &actor);
            }
            if names.len() == 12 || names.iter().map(|s| s.chars().count()).sum::<usize>() >= 239 {
                break;
            }
        }
        // Longer exact labels win at the matching site, e.g. a corroborated
        // three-character name ending in 先 must not be split at that character.
        names.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()).then(a.cmp(b)));
        self.actor_spellings = names;
        self
    }
    pub(crate) fn actor_spellings(&self) -> &[String] {
        &self.actor_spellings
    }
    pub(crate) fn actor_spellings_digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut names = self.actor_spellings.clone();
        names.sort();
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&("source-actor-spellings-v1", names)).unwrap())
        )
    }
    pub fn normalize(&self, text: &str) -> String {
        self.normalize_near_names(&self.normalize_exact(text))
    }
    fn normalize_exact(&self, text: &str) -> String {
        self.normalize_exact_with(text, |_, _| {})
    }
    fn normalize_exact_with(&self, text: &str, mut replaced: impl FnMut(&str, &str)) -> String {
        let Some(matcher) = &self.matcher else {
            return text.to_owned();
        };
        let mut result = String::new();
        let mut cursor = 0;
        for hit in matcher.find_iter(text) {
            let value = hit.as_str();
            let Some(name) = self.names.get(&value.to_lowercase()) else {
                // Regex Unicode case folding can match characters whose lowercase
                // spelling differs from the stored key. Leave those unchanged.
                continue;
            };
            let first = value.chars().next().unwrap();
            let last = value.chars().last().unwrap();
            if self.model_aliases.contains(&value.to_lowercase())
                && (text[..hit.start()]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                    || model_has_continuation(&text[hit.end()..]))
            {
                continue;
            }
            if first.is_ascii_alphanumeric()
                && text[..hit.start()]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
                || last.is_ascii_alphanumeric()
                    && text[hit.end()..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric())
            {
                continue;
            }
            result.push_str(&text[cursor..hit.start()]);
            result.push_str(name);
            if value != name {
                replaced(value, name);
            }
            cursor = hit.end();
        }
        result.push_str(&text[cursor..]);
        result
    }

    /// The bundled token inventory represents spoken digits as CJK characters.
    /// Keep the display spelling for notes, but send a complete encodable hint.
    pub fn recognition_hotwords(&self) -> Vec<String> {
        // Keep the existing model/alternative preference inside each rank.
        // Explicit punctuation aliases inherit their canonical name's rank.
        let mut ranked: [[Vec<String>; 5]; 3] =
            std::array::from_fn(|_| std::array::from_fn(|_| Vec::new()));
        for name in &self.hotwords {
            let [primary, other_models, title_fallbacks, alternatives, _] =
                &mut ranked[usize::from(self.priorities[name])];
            let mut seen_models = HashSet::new();
            let mut models = Vec::new();
            for (prefix, digits) in
                model_identifier(name)
                    .into_iter()
                    .chain(self.names.iter().filter_map(|(alias, canonical)| {
                        (canonical == name)
                            .then(|| model_identifier(alias))
                            .flatten()
                    }))
            {
                if seen_models.insert((prefix.to_ascii_lowercase(), digits.to_owned())) {
                    models.push((prefix, digits));
                }
            }
            if models.is_empty() {
                primary.push(name.clone());
            } else if model_identifier(name).is_none() && !name.chars().any(|c| c.is_ascii_digit())
            {
                title_fallbacks.push(name.clone());
            }
            for (i, (prefix, digits)) in models.iter().enumerate() {
                let hint = format!("{prefix} {}", spoken_digits(digits, '零'));
                if i == 0 {
                    primary.push(hint);
                } else {
                    other_models.push(hint);
                }
                alternatives.push(format!("{prefix} {}", english_digits(digits)));
            }
        }
        for (alias, canonical) in &self.recognition_aliases {
            ranked[usize::from(self.priorities[canonical])][4].push(alias.clone());
        }
        let mut seen = HashSet::new();
        ranked
            .into_iter()
            .flat_map(|groups| groups.into_iter().flatten())
            .filter(|word| seen.insert(word.to_lowercase()))
            .take(crate::asr_hotwords::MAX_WORDS)
            .collect()
    }
    fn normalize_near_names(&self, text: &str) -> String {
        self.normalize_near_names_with(text, |_, _| {})
    }
    fn normalize_near_names_with(
        &self,
        text: &str,
        mut replaced: impl FnMut(&str, &str),
    ) -> String {
        if self.approximate.is_empty() {
            return text.to_owned();
        }
        static WORD: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let words: Vec<_> = WORD
            .get_or_init(|| Regex::new(r"[A-Za-z0-9_]+").unwrap())
            .find_iter(text)
            .collect();
        let protected: Vec<_> = self
            .ambiguous_matcher
            .iter()
            .flat_map(|matcher| matcher.find_iter(text))
            .filter(|hit| {
                !(hit
                    .as_str()
                    .starts_with(|c: char| c.is_ascii_alphanumeric())
                    && text[..hit.start()]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_ascii_alphanumeric())
                    || hit.as_str().ends_with(|c: char| c.is_ascii_alphanumeric())
                        && text[hit.end()..]
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphanumeric()))
            })
            .map(|hit| hit.range())
            .collect();
        let mut out = String::new();
        let (mut cursor, mut i) = (0, 0);
        while i < words.len() {
            let mut matched = None;
            for count in (1..=3.min(words.len() - i)).rev() {
                let span = &words[i..i + count];
                if count > 1 && (span[0].len() < 3 || span[count - 1].len() < 3) {
                    continue;
                }
                if span.windows(2).any(|w| {
                    let gap = &text[w[0].end()..w[1].start()];
                    gap.is_empty() || gap.len() > 3 || !gap.chars().all(|c| c == ' ' || c == '-')
                }) {
                    continue;
                }
                let compact = span
                    .iter()
                    .map(|w| w.as_str())
                    .collect::<String>()
                    .to_ascii_lowercase();
                if compact.len() < 7 || !compact.bytes().all(|c| c.is_ascii_alphabetic()) {
                    continue;
                }
                let first_protected = protected.partition_point(|r| r.end <= span[0].start());
                if protected
                    .get(first_protected)
                    .is_some_and(|r| r.start < span[count - 1].end())
                    || !self.ambiguous_compacts.is_empty()
                        && (0..count).any(|start| {
                            (start + 1..=count).any(|end| {
                                let joined = span[start..end]
                                    .iter()
                                    .map(|word| word.as_str())
                                    .collect::<String>()
                                    .to_ascii_lowercase();
                                self.ambiguous_compacts.contains(&joined)
                            })
                        })
                {
                    continue;
                }
                // Never turn an already recognized *different* name into a nearby name.
                if let Some(name) = self.names.get(&compact) {
                    if count > 1 {
                        matched = Some((count, name));
                        break;
                    }
                    continue;
                }
                if count > 1
                    && span.iter().any(|word| {
                        let key = word.as_str().to_lowercase();
                        self.names
                            .get(&key)
                            .is_some_and(|name| name.to_lowercase() == key)
                    })
                {
                    continue;
                }
                let mut candidates: Vec<_> = self
                    .approximate
                    .iter()
                    .filter_map(|name| {
                        let key = name.to_ascii_lowercase();
                        if key.len().abs_diff(compact.len()) > 2 {
                            return None;
                        }
                        let shared = key.starts_with(&compact[..4])
                            || key.ends_with(&compact[compact.len() - 4..]);
                        let distance = edit_distance(compact.as_bytes(), key.as_bytes());
                        (shared && distance <= 2 && distance * 4 <= key.len())
                            .then_some((distance, name))
                    })
                    .collect();
                candidates.sort_by_key(|(distance, _)| *distance);
                if let Some((distance, name)) = candidates.first() {
                    if candidates.get(1).is_some_and(|(next, _)| next <= distance) {
                        continue;
                    }
                    matched = Some((count, *name));
                    break;
                }
            }
            if let Some((count, name)) = matched {
                let original = &text[words[i].start()..words[i + count - 1].end()];
                out.push_str(&text[cursor..words[i].start()]);
                out.push_str(name);
                if original != name {
                    replaced(original, name);
                }
                cursor = words[i + count - 1].end();
                i += count;
            } else {
                i += 1;
            }
        }
        out.push_str(&text[cursor..]);
        out
    }
    fn normalize_with_changes(&self, text: &str) -> (String, Vec<document::Replacement>) {
        let mut replacements = Vec::new();
        let mut record = |old: &str, new: &str| {
            if old.chars().any(char::is_alphabetic)
                && self
                    .writer_names
                    .iter()
                    .chain(&self.caption_names)
                    .any(|n| n == new)
            {
                replacements.push(document::Replacement {
                    old: old.into(),
                    new: new.into(),
                    model: self.model_aliases.contains(&old.to_lowercase()),
                });
            }
        };
        let exact = self.normalize_exact_with(text, &mut record);
        let normalized = self.normalize_near_names_with(&exact, record);
        (normalized, replacements)
    }
    /// A small spelling-only vocabulary survives context trimming and repair attempts.
    pub fn spellings(&self, transcript: &str) -> Vec<String> {
        let normalized = self.normalize(transcript).to_lowercase();
        let mut names = self.writer_names.clone();
        names.extend(source_owner_names(transcript));
        // Unspoken note words must not become invented meeting subjects.
        names.retain(|n| contains_name(&normalized, &n.to_lowercase()));
        names.sort();
        names.dedup();
        names.sort_by_key(|n| n.len());
        let mut budget = 240;
        names
            .into_iter()
            .filter(|n| {
                let size = n.chars().count();
                if size > budget {
                    return false;
                }
                budget -= size;
                true
            })
            .take(12)
            .collect()
    }
    pub(crate) fn caption_spellings(&self, transcript: &str) -> Vec<String> {
        let normalized = self.normalize(transcript).to_lowercase();
        self.caption_names
            .iter()
            .chain(&self.actor_spellings)
            .filter(|name| contains_name(&normalized, &name.to_lowercase()))
            .cloned()
            .chain(source_owner_names(transcript))
            .collect()
    }
    pub fn validate_writer_names(&self, transcript: &str, body: &str) -> Result<()> {
        crate::task_roles::validate(&self.normalize(transcript), &self.normalize_exact(body))?;
        crate::translation::validate_reported_name_spellings(transcript, body, |name| {
            self.normalize_exact(name)
        })?;
        let body = self.normalize_exact(body).to_lowercase();
        let owners = source_owner_names(transcript);
        // A sole explicit owner is a source spelling, not a guessed translation.
        // Multiple/changed ownership needs field identity; do not demand obsolete
        // owners merely because their names occurred in an earlier passage.
        static CHANGED: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let changed = CHANGED.get_or_init(|| Regex::new(r"(?i)no longer|not responsible|changed?|corrected|reassigned|不再负责|改由|转由|负责人.{0,12}(?:改|换|变)").unwrap());
        let sole_owner = owners.len() == 1 && !changed.is_match(transcript);
        let missing: Vec<_> = self
            .spellings(transcript)
            .into_iter()
            .filter(|name| self.writer_names.contains(name) || sole_owner && owners.contains(name))
            .filter(|name| !contains_name(&body, &name.to_lowercase()))
            .collect();
        if !missing.is_empty() {
            anyhow::bail!("纪要遗漏原话中的专有名称：{}。保留这些名称和对应原话的状态，不新增事实。Preserve these source names and their stated status.", missing.join(", "));
        }
        Ok(())
    }
    pub fn normalize_draft(&self, draft: &str) -> Result<String> {
        let mut value: serde_json::Value = serde_json::from_str(draft)?;
        if let Some(sections) = value["sections"].as_array_mut() {
            for section in sections {
                for field in ["title", "body"] {
                    if let Some(text) = section[field].as_str() {
                        section[field] = self.normalize(text).into();
                    }
                }
            }
        }
        Ok(serde_json::to_string(&value)?)
    }
}
/// Explicit ownership subjects only. Ordinary capitalized technical prose is
/// translatable; this does not create aliases or alter the original transcript.
fn source_owner_names(text: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static FUTURE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static RETAINED: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static MIXED: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static REPORTED: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re=RE.get_or_init(||Regex::new(r"\b(?P<name>[A-Z][A-Za-z'’-]{1,39}(?:\s+[A-Z][A-Za-z'’-]{1,39}){0,2})\s+(?:owns\b|is responsible for\b)").unwrap());
    let mut names = crate::task_roles::actors(text);
    // Title-case subjects with a stated future task are also name spellings.
    // Do not infer names from an all-caps ASR sentence ("BEEN APPROVED ALICE").
    let future=FUTURE.get_or_init(||Regex::new(r"\b(?P<name>[A-Z][a-z][A-Za-z'’-]{0,38}(?:\s+[A-Z][a-z][A-Za-z'’-]{0,38}){0,2})\s+will\s+(?:check|send|test|deliver|review|verify|publish|run|perform|complete|finish)\b").unwrap());
    // A later correction can retain a named owner's deadline without repeating
    // "owns". This is an explicit task subject, never a name inferred from a
    // note or a bare title-cased word elsewhere in the sentence.
    let retained=RETAINED.get_or_init(||Regex::new(r"\b(?P<name>[A-Z][a-z][A-Za-z'’-]{0,38}(?:\s+[A-Z][a-z][A-Za-z'’-]{0,38}){0,2})\s+(?:keeps|retains)\s+(?:the\s+)?(?:[A-Z][a-z]+\s+)?deadline\b").unwrap());
    // Chinese has no word boundary before an adjacent Latin name. Require an
    // explicit responsibility predicate and a full ASCII token; notes alone
    // never create an owner or an alias for a different name in the source.
    let mixed=MIXED.get_or_init(||Regex::new(r"(?:^|[^A-Za-z0-9_])(?P<name>[A-Z][A-Za-z'’-]{1,39}(?:[ \t]+[A-Z][a-z][A-Za-z'’-]{0,38}){0,2})[ \t]*(?:将[ \t]*)?负责").unwrap());
    // A reporting subject is a spelling, not proof of the reported claim or a
    // current assignment. Require a title-case, clause-leading label instead
    // of guessing a name inside an all-caps or unpunctuated ASR fragment.
    let reported=REPORTED.get_or_init(||Regex::new(r"(?:^|[.!?。！？；;\n])\s*(?P<name>[A-Z][a-z][A-Za-z'’-]{0,38}(?:[ \t]+[A-Z][a-z][A-Za-z'’-]{0,38}){0,2})\s+(?:confirms|confirmed|reports|reported)\s+\S").unwrap());
    for c in re
        .captures_iter(text)
        .chain(future.captures_iter(text))
        .chain(retained.captures_iter(text))
        .chain(mixed.captures_iter(text))
        .chain(reported.captures_iter(text))
    {
        if names.len() >= 12 {
            break;
        }
        let name = &c["name"];
        if [
            "We",
            "You",
            "They",
            "He",
            "She",
            "It",
            "Someone",
            "Anyone",
            "This",
            "That",
            "Team",
            "The Team",
            "Nobody",
            "Everybody",
            "Management",
            "System",
            "Server",
            "Model",
            "Application",
            "Device",
            "User",
            "Engineer",
            "Department",
            "The System",
            "The Server",
            "The Model",
            "The Application",
            "The Device",
            "The User",
            "The Department",
        ]
        .iter()
        .any(|word| word.eq_ignore_ascii_case(name))
            || name.split_whitespace().any(|word| {
                [
                    "If", "When", "Unless", "Maybe", "Perhaps", "On", "At", "By", "Before", "After",
                ]
                .iter()
                .any(|cue| cue.eq_ignore_ascii_case(word))
            })
        {
            continue;
        }
        if !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
        if names.len() == 12 {
            break;
        }
    }
    names
}
pub(crate) fn preserves_owner_spelling(source: &str, body: &str) -> bool {
    let body = body.to_lowercase();
    source_owner_names(source)
        .iter()
        .all(|n| contains_name(&body, &n.to_lowercase()))
}
fn contains_name(text: &str, name: &str) -> bool {
    let model = model_identifier(name).is_some();
    text.match_indices(name).any(|(start, value)| {
        !text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || model && c == '_')
            && if model {
                !model_has_continuation(&text[start + value.len()..])
            } else {
                !text[start + value.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
            }
    })
}
fn edit_distance(a: &[u8], b: &[u8]) -> usize {
    let mut row: Vec<_> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (previous + usize::from(x != y))
                .min(row[j] + 1)
                .min(old + 1);
            previous = old;
        }
    }
    row[b.len()]
}
impl Engine {
    pub fn terminology(&self, meeting: &str) -> Result<Terms> {
        let db = self.store.lock().unwrap();
        Ok(Terms::new(&db.notes(meeting)?, &db.terminology_entries()?))
    }
    /// Archive the original ASR revision. Human edits and protected minutes remain authoritative.
    pub fn correct_terminology(&self, meeting: &str) -> Result<usize> {
        self.correct_terminology_after_note(meeting, None)
    }
    pub(crate) fn correct_terminology_after_note(
        &self,
        meeting: &str,
        note_edit: Option<NoteSpellingEdit>,
    ) -> Result<usize> {
        #[cfg(test)]
        if let Some(pause) = BEFORE_CORRECTION_LOCK.with(|slot| slot.borrow_mut().take()) {
            pause();
        }
        let mut db = self.store.lock().unwrap();
        let changed = db.with_terminology_savepoint(|db| {
            // Derive the environment under the same guard used for all source,
            // citation and terms-key writes; an older worker cannot restore a
            // spelling snapshot captured before a concurrent note/glossary save.
            let terms = Terms::new(&db.notes(meeting)?, &db.terminology_entries()?);
            let mut m = db.meeting(meeting)?;
            let mut before_document = None;
            let mut document_sources = HashSet::new();
            let mut source_changes = Vec::new();
            let mut changed = 0;
            let terms_key = terms.fingerprint();
            db.schedule_terminology(&m, &terms_key)?;
            loop {
                let candidates = db.terminology_candidates(&m)?;
                if candidates.is_empty() {
                    break;
                }
                let mut checked = Vec::with_capacity(candidates.len());
                for mut s in candidates {
                    let origin: Option<String> = db.conn.query_row(
                    "SELECT origin FROM segment_text_edits WHERE id=?1 ORDER BY revision DESC LIMIT 1",
                    [&s.id], |r| r.get(0),
                ).optional()?;
                    if origin.as_deref() == Some("human") {
                        checked.push(s);
                        continue;
                    }
                    let original: Option<String> = db
                        .conn
                        .query_row(
                            "SELECT json FROM segment_history WHERE id=?1 ORDER BY revision LIMIT 1",
                            [&s.id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    let original: crate::Segment = original
                        .map(|s| serde_json::from_str(&s))
                        .transpose()?
                        .unwrap_or_else(|| s.clone());
                    // Legacy revisions lack provenance. Keep unknown edits; speaker-only changes and
                    // old exact-alias corrections can safely migrate. New edits carry explicit origin.
                    if origin.is_none()
                        && s.text != original.text
                        && terms.normalize_exact(&original.text) != s.text
                    {
                        checked.push(s);
                        continue;
                    }
                    let (text, replacements) = terms.normalize_with_changes(&original.text);
                    if text != s.text {
                        if before_document.is_none() {
                            let snapshot = db.suggestions(meeting)?;
                            document_sources = snapshot
                                .iter()
                                .filter(|section| section.status == "pending" && !section.stale)
                                .flat_map(|section| {
                                    section.sources.iter().map(|s| s.segment_id.clone())
                                })
                                .collect();
                            before_document = Some(snapshot);
                        }
                        let before = document_sources.contains(&s.id).then(|| s.clone());
                        db.edit_segment_text(&s.id, &text, s.revision, "terminology")?;
                        s.revision += 1;
                        s.text = text;
                        if let Some(before) = before {
                            // The authoritative target always comes from archived ASR.
                            // A later spelling edit must replay from the revision the
                            // article actually cites, rather than an older alias that
                            // disappeared during an earlier automatic correction.
                            let (current, current_replacements) =
                                terms.normalize_with_changes(&before.text);
                            let replacements = if current == s.text {
                                current_replacements
                            } else {
                                replacements
                            };
                            source_changes.push(document::SourceChange {
                                before,
                                after: s.clone(),
                                replacements,
                            });
                        }
                        changed += 1;
                    }
                    checked.push(s);
                }
                db.mark_terminology_batch(&checked, &terms_key)?;
            }
            db.set_cache_dependency(meeting, "terms", &terms_key)?;
            if changed > 0 {
                let reusable = if let (Some(edit), Some(before)) = (&note_edit, &before_document) {
                    edit.reusable_fingerprint(
                        &m,
                        &db.notes(meeting)?,
                        before,
                        &db.segments(&m)?,
                        &source_changes,
                        changed,
                    )?
                } else {
                    None
                };
                m.notes_fingerprint.clear();
                db.save_meeting(&m)?;
                for mut s in db.suggestions(meeting)? {
                    if s.status == "pending" {
                        // Regenerate against corrected, revisioned evidence; never patch prose
                        // while leaving its citations attached to the old words.
                        s.stale = true;
                        db.save_suggestion(&s)?;
                    }
                }
                if let Some(before) = before_document {
                    db.repair_terminology_documents(&m, &before, &source_changes, reusable)?;
                }
            }
            Ok(changed)
        })?;
        drop(db);
        if changed > 0 {
            self.sync_meeting_files(meeting);
        }
        Ok(changed)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_person_spelling_changes_are_rejected_without_requiring_every_history_name() {
        let terms = Terms::new(&[], &[]);
        for (source, changed) in [
            (
                "北岭大学的 LENA FARO森教授指出公开照片可能有风险。",
                "北岭大学的 LENA FARO 森教授指出公开照片可能有风险。",
            ),
            (
                "研究所的沈 MALE DUNSTON博士表示报告仍未确认。",
                "研究所的 SEN MALE DUNSTON博士表示报告仍未确认。",
            ),
        ] {
            assert!(
                terms.validate_writer_names(source, changed).is_err(),
                "{changed}"
            );
            assert!(terms.validate_writer_names(source, source).is_ok());
        }
    }

    #[test]
    fn reported_names_allow_only_observed_or_explicitly_mapped_spellings_without_hotwords() {
        let source = "LENA FARO森教授指出报告仍未确认。";
        let plain = Terms::new(&[], &[]);
        assert!(plain.spellings(source).is_empty());
        assert!(plain.actor_spellings().is_empty());
        assert!(plain.recognition_hotwords().is_empty());
        assert_eq!(plain.normalize(source), source);
        plain
            .validate_writer_names(
                source,
                "Professor LENA FARO森 stated that the report is pending.",
            )
            .unwrap();
        plain
            .validate_writer_names(source, "报告仍未确认。")
            .unwrap();
        let mapped = Terms::new(
            &[],
            &[Entry {
                kind: "term".into(),
                title: "Lena Faro".into(),
                aliases: "LENA FARO森".into(),
                enabled: true,
                ..Default::default()
            }],
        );
        mapped
            .validate_writer_names(
                source,
                "Professor Lena Faro stated that the report is pending.",
            )
            .unwrap();
        let unrelated = Terms::new(
            &[],
            &[Entry {
                kind: "term".into(),
                title: "Zora Nil".into(),
                enabled: true,
                ..Default::default()
            }],
        );
        assert!(unrelated
            .validate_writer_names(
                source,
                "Professor Zora Nil stated that the report is pending."
            )
            .is_err());
        let history = "LENA FARO森教授指出原报告待确认。该报告已经撤回。沈 MALE DUNSTON博士表示新报告仍待确认。";
        plain
            .validate_writer_names(
                history,
                "Doctor 沈 MALE DUNSTON reported that the new report is pending.",
            )
            .unwrap();
    }

    #[test]
    fn reported_literal_names_do_not_authorize_conditional_or_quoted_speakers() {
        let terms = Terms::new(&[], &[]);
        for source in [
            "LENA FARO森教授指出记者如果公布照片可能有风险。",
            "LENA FARO森教授指出‘记者如果公布照片可能有风险’。",
            "LENA FARO森教授并未指出记者如果公布照片可能有风险。",
        ] {
            let body = format!("{source}记者表示需要继续核实。");
            terms.validate_writer_names(source, &body).unwrap();
            let segment = crate::Segment {
                id: "m:1:1".into(),
                meeting_id: "m".into(),
                run: 1,
                seq: 1,
                start_ms: 0,
                end_ms: 1000,
                text: source.into(),
                revision: 1,
                speaker: None,
                speaker_manual: false,
            };
            let draft =
                serde_json::json!({"sections":[{"title":"纪要","body":body,"sources":[1]}]})
                    .to_string();
            let error = crate::notes::validate(&draft, &[segment], "m").unwrap_err();
            assert!(error.is::<crate::notes::AttributionMismatch>(), "{error:#}");
        }
    }

    fn concurrent_context_save_cannot_revert_sources(update_term: bool) -> Result<()> {
        use crate::{Segment, Source, Suggestion};
        use std::{sync::mpsc, thread, time::Duration};

        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Concurrent spelling update")?;
        engine.update_meeting(&meeting.id, &meeting.title, false)?;
        let note = engine.save_note(&meeting.id, None, "CEDARLEDGER", "document", 0)?;
        let raw = Segment {
            id: format!("{}:1:1", meeting.id),
            meeting_id: meeting.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "CEDARLEDGER尚未完成，预算仍未批准。".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        let mut human = raw.clone();
        human.id = format!("{}:1:2", meeting.id);
        human.seq = 2;
        human.start_ms = 1000;
        human.end_ms = 2000;
        {
            let db = engine.store.lock().unwrap();
            db.save_segment(&raw)?;
            db.save_segment(&human)?;
        }
        engine.edit_segment(&human.id, "人工确认：CEDARLEDGER仅是示例。", 1)?;
        let before = engine.snapshot(&meeting.id)?;
        let section = Suggestion {
            focus: String::new(),
            id: String::new(),
            meeting_id: meeting.id.clone(),
            kind: "section".into(),
            text: raw.text.clone(),
            sources: vec![Source {
                segment_id: raw.id.clone(),
                revision: raw.revision,
                start_ms: raw.start_ms,
                end_ms: raw.end_ms,
                quote: raw.text.clone(),
                speaker_name: None,
            }],
            status: "pending".into(),
            stale: false,
        };
        let mut accepted = section.clone();
        accepted.status = "accepted".into();
        accepted.text = "人工纪要：CEDARLEDGER仍保留为示例。".into();
        engine.store.lock().unwrap().commit_document(
            &before,
            &before.segments,
            2,
            vec![section, accepted],
            false,
        )?;
        let other = engine.create_meeting("Unrelated meeting")?;
        engine.save_note(&other.id, None, "OtherProduct", "document", 0)?;
        let mut other_source = raw.clone();
        other_source.id = format!("{}:1:1", other.id);
        other_source.meeting_id = other.id.clone();
        other_source.text = "OtherProduct仍待确认。".into();
        engine.store.lock().unwrap().save_segment(&other_source)?;

        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker_engine = engine.clone();
        let worker_meeting = meeting.id.clone();
        let worker = thread::spawn(move || {
            BEFORE_CORRECTION_LOCK.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move || {
                    entered_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                }));
            });
            worker_engine.correct_terminology(&worker_meeting)
        });
        // In the old path the worker has already captured A's Terms here.
        // Resume only after the real public save has completely applied B.
        entered_rx.recv_timeout(Duration::from_secs(10))?;
        let saved = if update_term {
            engine
                .save_library_entry(Entry {
                    kind: "term".into(),
                    title: "CedarLedger".into(),
                    aliases: "CEDARLEDGER".into(),
                    ..Default::default()
                })
                .map(|_| ())
        } else {
            engine
                .save_note(&meeting.id, Some(note.id), "CedarLedger", "document", 0)
                .map(|_| ())
        };
        let expected = engine.snapshot(&meeting.id)?;
        let other_expected = engine.snapshot(&other.id)?;
        let expected_key = engine
            .store
            .lock()
            .unwrap()
            .cache_dependency(&meeting.id, "terms")?;
        let other_key = engine
            .store
            .lock()
            .unwrap()
            .cache_dependency(&other.id, "terms")?;
        // Release even when the public save failed, so the regression never
        // strands a waiting worker or relies on a sleep to race the mutex.
        resume_tx.send(())?;
        let corrected = worker.join().expect("correction worker must not panic")?;
        saved?;

        let after = engine.snapshot(&meeting.id)?;
        let after_key = engine
            .store
            .lock()
            .unwrap()
            .cache_dependency(&meeting.id, "terms")?;
        assert_eq!(
            expected.segments[0].text,
            "CedarLedger尚未完成，预算仍未批准。"
        );
        assert_eq!(expected.segments[0].revision, 2);
        assert_eq!(expected.suggestions[0].text, expected.segments[0].text);
        assert_eq!(
            expected.suggestions[0].sources[0].quote,
            expected.segments[0].text
        );
        assert_eq!(expected.suggestions[0].sources[0].revision, 2);
        assert_eq!(
            serde_json::json!({"sources":after.segments,"minutes":after.suggestions,"terms":after_key}),
            serde_json::json!({"sources":expected.segments,"minutes":expected.suggestions,"terms":expected_key}),
            "an older worker must not roll back the current spelling, citation or terms key"
        );
        assert_eq!(corrected, 0);
        assert_eq!(engine.terminology(&meeting.id)?.fingerprint(), expected_key);
        assert_eq!(after.segments[1].text, "人工确认：CEDARLEDGER仅是示例。");
        assert_eq!(after.segments[1].revision, 2);
        assert_eq!(
            after.suggestions[1].text,
            "人工纪要：CEDARLEDGER仍保留为示例。"
        );
        assert_eq!(after.suggestions[1].status, "accepted");
        let other_after = engine.snapshot(&other.id)?;
        assert_eq!(
            serde_json::to_value(&other_after.segments)?,
            serde_json::to_value(&other_expected.segments)?
        );
        assert_eq!(
            engine
                .store
                .lock()
                .unwrap()
                .cache_dependency(&other.id, "terms")?,
            other_key
        );
        let db = engine.store.lock().unwrap();
        let archived: String = db.conn.query_row(
            "SELECT json FROM segment_history WHERE id=?1 AND revision=1",
            [&raw.id],
            |row| row.get(0),
        )?;
        assert_eq!(serde_json::from_str::<Segment>(&archived)?.text, raw.text);
        let revisions: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM segment_history WHERE id=?1",
            [&raw.id],
            |row| row.get(0),
        )?;
        assert_eq!(revisions, 1);
        let jobs: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM inference_jobs", [], |row| row.get(0))?;
        assert_eq!(jobs, 0);
        Ok(())
    }

    #[test]
    fn concurrent_note_save_cannot_restore_an_older_terminology_snapshot() -> Result<()> {
        concurrent_context_save_cannot_revert_sources(false)
    }

    #[test]
    fn concurrent_glossary_save_cannot_restore_an_older_terminology_snapshot() -> Result<()> {
        concurrent_context_save_cannot_revert_sources(true)
    }

    #[test]
    fn combined_chinese_and_english_task_frames_keep_the_twelve_name_bound() {
        let mut text = (b'A'..=b'L')
            .map(|c| format!("任务由Actor{}负责。", char::from(c)))
            .collect::<String>();
        text.push_str(" Nora will check the model.");
        assert_eq!(source_owner_names(&text).len(), 12);
    }
    #[test]
    fn reporting_and_executable_task_actors_preserve_observed_spellings_only() {
        let terms = Terms::new(&[], &[]);
        for (source, actor) in [
            ("Riley will run the invoice review on Friday.", "Riley"),
            ("Riley will perform the invoice review on Friday.", "Riley"),
            ("Nora will complete the model check.", "Nora"),
            ("Nora will finish the model check.", "Nora"),
            (
                "Morgan confirms the upload verification has now been completed.",
                "Morgan",
            ),
            (
                "Morgan reported that the upload verification is still pending.",
                "Morgan",
            ),
            ("离线转写工具由林悦负责测试。", "林悦"),
        ] {
            assert_eq!(terms.caption_spellings(source), [actor], "{source}");
            assert!(
                !preserves_owner_spelling(source, "任务仍待处理。"),
                "{source}"
            );
            assert_eq!(terms.normalize(source), source);
        }
        for source in [
            "We will run the check.",
            "The Team will perform the check.",
            "System confirms the upload is complete.",
            "The Model reports the result.",
            "RILEY WILL RUN THE INVOICE REVIEW.",
            "MORGAN CONFIRMS THE UPLOAD IS COMPLETE.",
            "If Morgan confirms the upload, continue.",
            "On Thursday Riley will run the check.",
            "The model will run the check.",
        ] {
            assert!(terms.caption_spellings(source).is_empty(), "{source}");
        }
    }
    #[test]
    fn corroborated_actor_context_keeps_exact_long_names_without_date_guessing() {
        let first = "离线转写工具由张敬先负责测试。";
        let date = "张敬先在周四提交报告。";
        let plain = Terms::new(&[], &[]);
        assert!(plain.caption_spellings(date).is_empty());
        let fingerprint = plain.fingerprint();
        let empty_digest = plain.actor_spellings_digest();
        let terms = plain.with_actor_spellings([first, date], std::iter::empty());
        assert_eq!(terms.actor_spellings(), ["张敬先"]);
        assert_eq!(terms.caption_spellings(date), ["张敬先"]);
        assert_eq!(terms.normalize(date), date);
        assert_eq!(terms.fingerprint(), fingerprint);
        assert_ne!(terms.actor_spellings_digest(), empty_digest);
        let reverse = Terms::new(&[], &[]).with_actor_spellings([date, first], std::iter::empty());
        assert_eq!(
            terms.actor_spellings_digest(),
            reverse.actor_spellings_digest()
        );

        let terms = Terms::new(&[], &[])
            .with_actor_spellings(["林悦先在周四提交测试报告。"], ["林悦", "NotSpoken"]);
        assert_eq!(terms.actor_spellings(), ["林悦"]);
        assert_eq!(
            terms.caption_spellings("林悦先在周四提交测试报告。"),
            ["林悦"]
        );
        assert!(terms.caption_spellings("这次只讨论测试报告。").is_empty());
        assert_eq!(terms.normalize("Lin Yuejian"), "Lin Yuejian");
    }
    #[test]
    fn actor_context_stays_bounded_and_does_not_revive_an_old_owner() {
        let source = (b'A'..=b'Z')
            .map(|c| format!("测试任务由Actor{}负责检查。", char::from(c)))
            .collect::<Vec<_>>();
        let terms = Terms::new(&[], &[])
            .with_actor_spellings(source.iter().map(String::as_str), std::iter::empty());
        assert_eq!(terms.actor_spellings().len(), 12);
        assert!(
            terms
                .actor_spellings()
                .iter()
                .map(|s| s.chars().count())
                .sum::<usize>()
                <= 240
        );
        let labels: Vec<_> = (0..12)
            .map(|i| format!("Person{i:02}{}", "x".repeat(72)))
            .collect();
        let terms = Terms::new(&[], &[]).with_actor_spellings(
            labels.iter().map(String::as_str),
            labels.iter().map(String::as_str),
        );
        assert!(terms.actor_spellings().len() <= 3);
        assert!(
            terms
                .actor_spellings()
                .iter()
                .map(|s| s.chars().count())
                .sum::<usize>()
                <= 240
        );

        let old = "模型下载由李明负责检查。";
        let new = "模型下载由王华负责检查，李明不再负责。";
        let terms = Terms::new(&[], &[]).with_actor_spellings([old, new], std::iter::empty());
        assert_eq!(terms.caption_spellings("王华负责检查。"), ["王华"]);
        assert!(terms
            .validate_writer_names(&format!("{old}{new}"), "王华负责检查。")
            .is_ok());
    }
    #[test]
    fn mixed_script_responsibility_preserves_only_stated_current_names() {
        let terms = Terms::new(&[note("Alice负责检查")], &[]);
        let source = "Alice负责检查显卡。";
        assert!(
            terms
                .validate_writer_names(source, "负责检查显卡。")
                .is_err(),
            "the writer must not silently drop the explicit task owner"
        );
        assert_eq!(terms.spellings(source), ["Alice"]);
        assert!(terms.caption_spellings(source).iter().any(|n| n == "Alice"));
        assert!(terms
            .validate_writer_names(source, "Alice负责检查显卡。")
            .is_ok());
        assert!(!preserves_owner_spelling(
            source,
            "Check the graphics card."
        ));
        assert!(preserves_owner_spelling(
            source,
            "Alice will check the graphics card."
        ));
        assert_eq!(terms.normalize(source), source);

        let other = "请Bob负责检查显卡。";
        assert_eq!(terms.spellings(other), ["Bob"]);
        assert!(terms
            .validate_writer_names(other, "Bob负责检查显卡。")
            .is_ok());
        assert!(terms
            .validate_writer_names(other, "Alice负责检查显卡。")
            .is_err());
        for ordinary in [
            "Alice的显卡需要检查。",
            "WE负责检查显卡。",
            "The Team负责检查显卡。",
            "preAlice负责检查显卡。",
        ] {
            assert!(terms.spellings(ordinary).is_empty(), "{ordinary}");
        }
        for withdrawn in [
            "Alice不再负责检查显卡，改由Bob接手。",
            "Alice负责检查显卡，现在改由Bob接手。",
        ] {
            assert!(
                terms
                    .validate_writer_names(withdrawn, "Bob接手显卡检查。")
                    .is_ok(),
                "{withdrawn}"
            );
        }
    }
    #[test]
    fn explicit_source_owner_keeps_spelling_without_guessing_transliterations_or_old_owners() {
        let terms = Terms::new(&[], &[]);
        let source="The offline recorder is ready for testing. Morgan owns the latency report and will deliver it on Thursday.";
        assert_eq!(terms.spellings(source), ["Morgan"]);
        assert!(terms
            .validate_writer_names(source, "Morgan 周四交付延迟报告。")
            .is_ok());
        assert!(terms
            .validate_writer_names(source, "莫根周四交付延迟报告。")
            .is_err());
        assert_eq!(terms.normalize(source), source);
        assert!(terms
            .spellings("The offline prototype is ready for review. We will test it on Friday.")
            .is_empty());
        let changed =
            "Morgan owns the latency report. Ownership changed: Riley owns the latency report.";
        assert!(terms
            .validate_writer_names(changed, "Riley 周四交付延迟报告。")
            .is_ok());
    }
    #[test]
    fn unchanged_terminology_only_rechecks_new_or_revised_passages() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Incremental terminology")?;
        for seq in 1..=300 {
            engine.store.lock().unwrap().save_segment(&crate::Segment {
                id: format!("source-{seq}"),
                meeting_id: meeting.id.clone(),
                run: 1,
                seq,
                start_ms: u64::from(seq) * 1000,
                end_ms: u64::from(seq + 1) * 1000,
                text: "预算尚未确定。".into(),
                revision: 1,
                speaker: None,
                speaker_manual: false,
            })?;
        }
        assert_eq!(
            engine
                .store
                .lock()
                .unwrap()
                .terminology_candidates(&meeting)?
                .len(),
            128
        );
        engine.correct_terminology(&meeting.id)?;
        assert!(engine
            .store
            .lock()
            .unwrap()
            .terminology_candidates(&meeting)?
            .is_empty());
        // Late insertion before the previous maximum seq must not be skipped by a cursor.
        let source = crate::Segment {
            id: "late-source".into(),
            meeting_id: meeting.id.clone(),
            run: 1,
            seq: 0,
            start_ms: 0,
            end_ms: 1000,
            text: "尚未批准。".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        engine.store.lock().unwrap().save_segment(&source)?;
        assert_eq!(
            engine
                .store
                .lock()
                .unwrap()
                .terminology_candidates(&meeting)?
                .len(),
            1
        );
        engine.correct_terminology(&meeting.id)?;
        engine
            .store
            .lock()
            .unwrap()
            .edit_segment("late-source", "人工文字", 1)?;
        assert_eq!(
            engine
                .store
                .lock()
                .unwrap()
                .terminology_candidates(&meeting)?
                .len(),
            1
        );
        engine.correct_terminology(&meeting.id)?;
        assert!(engine
            .store
            .lock()
            .unwrap()
            .terminology_candidates(&meeting)?
            .is_empty());
        assert_eq!(engine.snapshot(&meeting.id)?.segments[0].text, "人工文字");
        drop(engine);
        let reopened = Engine::open(root.path().into())?;
        assert!(reopened
            .store
            .lock()
            .unwrap()
            .terminology_candidates(&meeting)?
            .is_empty());
        Ok(())
    }
    #[test]
    fn future_task_owner_spelling_is_preserved_without_guessing_all_caps_subjects() {
        let terms = Terms::new(&[], &[]);
        let source =
            "The budget has not been approved. Alice will check the NVIDIA GTX1650 local model.";
        assert_eq!(terms.spellings(source), ["Alice"]);
        assert!(!preserves_owner_spelling(
            source,
            "艾莉丝将检查NVIDIA GTX1650本地模型。"
        ));
        assert!(preserves_owner_spelling(
            source,
            "Alice将检查NVIDIA GTX1650本地模型。"
        ));
        assert!(terms
            .validate_writer_names(source, "艾莉丝将检查显卡。")
            .is_err());
        assert!(terms
            .spellings("THE BUDGET HAS NOT BEEN APPROVED ALICE WILL CHECK THE UN VIDEO CARD")
            .is_empty());
        assert!(terms.spellings("We will check the recorder.").is_empty());
        let source = "The budget is corrected to 5000. Morgan keeps the Thursday deadline.";
        assert_eq!(terms.caption_spellings(source), ["Morgan"]);
        assert!(preserves_owner_spelling(
            source,
            "5000，Morgan保留周四期限。"
        ));
        assert!(!preserves_owner_spelling(
            source,
            "5000，莫根保留周四期限。"
        ));
        assert!(terms
            .spellings("The Budget keeps its current amount.")
            .is_empty());
    }
    #[test]
    fn disabling_a_term_reverts_only_automatic_corrections() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Term disable")?;
        for seq in 1..=2 {
            engine.store.lock().unwrap().save_segment(&crate::Segment {
                id: format!("source-{seq}"),
                meeting_id: meeting.id.clone(),
                run: 1,
                seq,
                start_ms: 0,
                end_ms: 1000,
                text: "Project zargle is pending".into(),
                revision: 1,
                speaker: None,
                speaker_manual: false,
            })?;
        }
        let mut entry = engine.save_library_entry(crate::library::Entry {
            kind: "term".into(),
            title: "Zargle".into(),
            aliases: "zargle".into(),
            ..Default::default()
        })?;
        let source = engine.snapshot(&meeting.id)?.segments[0].clone();
        assert_eq!(source.text, "Project Zargle is pending");
        engine.edit_segment(&source.id, "Human wording: Zargle", source.revision)?;
        let before = engine
            .store
            .lock()
            .unwrap()
            .cache_dependency(&meeting.id, "terms")?;
        entry.enabled = false;
        engine.save_library_entry(entry)?;
        let snapshot = engine.snapshot(&meeting.id)?;
        assert_eq!(snapshot.segments[0].text, "Human wording: Zargle");
        assert_eq!(snapshot.segments[1].text, "Project zargle is pending");
        assert_ne!(
            before,
            engine
                .store
                .lock()
                .unwrap()
                .cache_dependency(&meeting.id, "terms")?
        );
        Ok(())
    }
    #[test]
    fn hiding_and_restoring_linked_notes_updates_spelling_context() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Linked spelling")?;
        engine.store.lock().unwrap().save_segment(&crate::Segment {
            id: "linked-source".into(),
            meeting_id: meeting.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "测试 OMA SCRIBE".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        })?;
        let note = engine.save_note(&meeting.id, None, "测试omascribe", "document", 0)?;
        assert_eq!(
            engine.snapshot(&meeting.id)?.segments[0].text,
            "测试 OmaScribe"
        );
        let mut entry = engine
            .library_entries()?
            .into_iter()
            .find(|e| e.id == note.id)
            .unwrap();
        entry.deleted = true;
        engine.save_library_entry(entry.clone())?;
        assert_eq!(
            engine.snapshot(&meeting.id)?.segments[0].text,
            "测试 OMA SCRIBE"
        );
        entry.deleted = false;
        engine.save_library_entry(entry)?;
        assert_eq!(
            engine.snapshot(&meeting.id)?.segments[0].text,
            "测试 OmaScribe"
        );
        Ok(())
    }
    fn note(content: &str) -> UserNote {
        UserNote {
            id: "n".into(),
            meeting_id: "m".into(),
            kind: "document".into(),
            content: content.into(),
            anchor_ms: 0,
            updated_at: String::new(),
        }
    }

    fn document_asr_admission_reaches_wire(content: &str, expected: &str) -> Result<()> {
        let directory = tempfile::tempdir()?;
        let engine = Engine::open(directory.path().into())?;
        let meeting = engine.create_meeting("Marked ASR admission")?;
        engine.update_meeting(&meeting.id, &meeting.title, false)?;
        let before = engine.terminology(&meeting.id)?.fingerprint();
        let saved = engine.save_note(&meeting.id, None, content, "document", 0)?;
        assert_eq!(saved.content, content);
        let terms = engine.terminology(&meeting.id)?;
        let wire = crate::asr_hotwords::encode_words(&terms.recognition_hotwords());
        assert_eq!(wire.words, expected);
        assert_eq!(crate::asr_hotwords::validate_words(&wire.words)?, expected);
        assert_eq!(
            terms.fingerprint(),
            before,
            "ASR-only hints do not create name mappings"
        );
        Ok(())
    }

    #[test]
    fn document_marked_asr_admission_chandra_reaches_wire() -> Result<()> {
        document_asr_admission_reaches_wire(
            "**Chandra Shekhar Solanki**",
            "CHANDRA SHEKHAR SOLANKI",
        )
    }

    #[test]
    fn document_marked_asr_admission_lakkha_reaches_wire() -> Result<()> {
        document_asr_admission_reaches_wire("**Lakkha Singh**", "LAKKHA SINGH")
    }

    #[test]
    fn document_marked_asr_admission_short_identifier_reaches_wire() -> Result<()> {
        document_asr_admission_reaches_wire("**F1**", "F1")
    }

    #[test]
    fn invalid_marked_span_does_not_consume_a_following_complete_hint() {
        for invalid in ["A".repeat(81), String::new(), "é".into(), "éé".into()] {
            let content = format!("**{invalid}** **Lakkha Singh**");
            assert_eq!(note_asr_marked_spellings(&content), ["Lakkha Singh"]);
        }
        assert!(Terms::new(&[note("**é** **éé**")], &[])
            .recognition_hotwords()
            .is_empty());
    }

    #[test]
    fn frozen_name_calibration_documents_reach_the_complete_asr_wire() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/hotword-calibration-en.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 100);
        let mut missing = Vec::new();
        for case in cases {
            for (arm, field) in [
                ("positive", "expected_name"),
                ("negative", "forbidden_name"),
            ] {
                let note = note(case[arm]["note"].as_str().unwrap());
                let expected = case[arm][field].as_str().unwrap().to_uppercase();
                let terms = Terms::new(&[note], &[]);
                let wire = crate::asr_hotwords::encode_words(&terms.recognition_hotwords());
                assert!(crate::asr_hotwords::validate_words(&wire.words).is_ok());
                if !wire.words.lines().any(|word| word == expected) {
                    missing.push(format!(
                        "{} {arm}: {expected}",
                        case["name"].as_str().unwrap()
                    ));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "Frozen document admission misses: {missing:?}"
        );
    }

    #[test]
    fn marked_asr_only_spans_do_not_become_name_or_fact_permissions() {
        let empty = Terms::new(&[], &[]);
        for (content, expected) in [
            ("**Chandra Shekhar Solanki**", "Chandra Shekhar Solanki"),
            ("**Cabo da Roca district**", "Cabo da Roca district"),
            ("**University of São Paulo**", "University of São Paulo"),
            ("**New  York**", "New  York"),
            ("**Budget review**", "Budget review"),
            ("**F1**", "F1"),
            ("**Chişinău**", "Chişinău"),
            ("**ASUNCIÓN**", "ASUNCIÓN"),
        ] {
            let terms = Terms::new(&[note(content)], &[]);
            assert!(
                terms.recognition_hotwords().contains(&expected.to_owned()),
                "{content}"
            );
            assert_eq!(terms.fingerprint(), empty.fingerprint(), "{content}");
            assert!(terms.writer_names.is_empty(), "{content}");
            assert!(terms.caption_names.is_empty(), "{content}");
            assert!(terms.approximate.is_empty(), "{content}");
            assert!(terms.names.is_empty(), "{content}");
            for source in [
                "Chandra is speaking.",
                "CHANDRA is speaking.",
                "CHANDRA SHEKHAR SOLANKI remains unconfirmed.",
                "The Budget review is not approved.",
            ] {
                assert_eq!(terms.normalize(source), source, "{content}");
                assert_eq!(
                    crate::translation::source_name_mask(source, source, &terms),
                    crate::translation::source_name_mask(source, source, &empty),
                    "{content}"
                );
            }
        }
        let terms = Terms::new(&[note("**Budget review**")], &[]);
        assert!(terms
            .validate_writer_names("The Budget review is not approved.", "预算审查未获批准。")
            .is_ok());
    }

    #[test]
    fn marked_unicode_words_do_not_leave_partial_ascii_names_or_hints() {
        let terms = Terms::new(&[note("**Chişinău** **CHIŞINĂU** **ASUNCIÓN**")], &[]);
        let wire = crate::asr_hotwords::encode_words(&terms.recognition_hotwords());
        assert_eq!(
            wire.words.lines().collect::<Vec<_>>(),
            ["ASUNCIÓN", "CHIŞINĂU"]
        );
        assert!(terms.names.is_empty());
        assert!(terms.writer_names.is_empty());
        assert!(terms.caption_names.is_empty());
        assert!(terms.approximate.is_empty());
        assert_eq!(
            terms.normalize("CHI and ASUNCI remain unconfirmed."),
            "CHI and ASUNCI remain unconfirmed."
        );
        let separate = Terms::new(&[note("CHI\n**CHIŞINĂU**")], &[]);
        assert_eq!(separate.normalize("chi"), "CHI");
        assert!(separate.hotwords.contains(&"CHI".to_owned()));
        let mut declared = note("ExplicitTerm");
        declared.kind = "term".into();
        assert_eq!(
            Terms::new(&[declared], &[]).normalize("EXPLICITTERM"),
            "ExplicitTerm"
        );
    }

    #[test]
    fn unmarked_generic_malformed_or_native_control_text_adds_no_new_asr_span() {
        for content in [
            "Chandra Shekhar Solanki",
            "**offline transcription**",
            "**in**",
            "**11**",
            "**F**",
            "**New\nYork**",
            "**New\tYork**",
            "**New York :99999**",
            "**New York\0**",
            "**New York-**",
        ] {
            assert!(note_asr_marked_spellings(content).is_empty(), "{content}");
        }
        for content in [
            "Chandra Shekhar Solanki",
            "**offline transcription**",
            "**in**",
        ] {
            assert!(
                Terms::new(&[note(content)], &[])
                    .recognition_hotwords()
                    .is_empty(),
                "{content}"
            );
        }
        assert_eq!(
            Terms::new(&[note("**FBI**")], &[]).recognition_hotwords(),
            ["FBI"]
        );
        assert!(note_asr_marked_spellings("**GTX 1650**").is_empty());
        assert_eq!(
            Terms::new(&[note("**GTX 1650**")], &[]).recognition_hotwords(),
            Terms::new(&[note("GTX 1650")], &[]).recognition_hotwords()
        );
    }

    #[test]
    fn marked_asr_only_spans_keep_note_priority_and_explicit_glossary_ownership() {
        let entries = glossary(80);
        let plain = Terms::new(&[], &entries);
        let terms = Terms::new(&[note("**Chandra Shekhar Solanki**")], &entries);
        let hints = terms.recognition_hotwords();
        assert_eq!(hints.len(), 80);
        assert_eq!(hints[0], "Chandra Shekhar Solanki");
        assert_eq!(terms.fingerprint(), plain.fingerprint());
        let mut entries = entries;
        entries.push(Entry {
            kind: "term".into(),
            title: "Node.js".into(),
            aliases: "Chandra Shekhar Solanki".into(),
            ..Default::default()
        });
        let explicit = Terms::new(&[note("**Chandra Shekhar Solanki**")], &entries);
        assert_eq!(explicit.normalize("CHANDRA SHEKHAR SOLANKI"), "Node.js");
        assert_eq!(
            explicit.recognition_hotwords()[0],
            "Chandra Shekhar Solanki"
        );
        entries.push(Entry {
            kind: "term".into(),
            title: "Other.Name".into(),
            aliases: "Chandra Shekhar Solanki".into(),
            ..Default::default()
        });
        let conflicts = Terms::new(&[note("**Chandra Shekhar Solanki**")], &entries);
        assert_eq!(
            conflicts.normalize("CHANDRA SHEKHAR SOLANKI"),
            "CHANDRA SHEKHAR SOLANKI"
        );
        assert!(!conflicts
            .recognition_hotwords()
            .iter()
            .any(|word| word.eq_ignore_ascii_case("Chandra Shekhar Solanki")));
        entries.last_mut().unwrap().enabled = false;
        let enabled = Terms::new(&[note("**Chandra Shekhar Solanki**")], &entries);
        assert_eq!(enabled.normalize("CHANDRA SHEKHAR SOLANKI"), "Node.js");
    }

    #[test]
    fn marked_asr_spans_share_the_unchanged_word_and_complete_utf8_byte_budgets() {
        let names: Vec<_> = (0..80)
            .map(|i| {
                format!(
                    "{} {}{}{}",
                    "Ａ".repeat(39),
                    "Ｂ".repeat(38),
                    char::from_u32(0xff10 + i / 10).unwrap(),
                    char::from_u32(0xff10 + i % 10).unwrap()
                )
            })
            .collect();
        assert!(names.iter().all(|name| name.chars().count() == 80));
        let content = names
            .iter()
            .map(|name| format!("**{name}**"))
            .collect::<Vec<_>>()
            .join("\n");
        let terms = Terms::new(&[note(&content)], &[]);
        let hints = terms.recognition_hotwords();
        assert_eq!(hints.len(), crate::asr_hotwords::MAX_WORDS);
        let wire = crate::asr_hotwords::encode_words(&hints);
        assert_eq!(wire.stats.accepted_terms, 66);
        assert_eq!(wire.stats.dropped_by_reason.byte_limit, 14);
        assert!(wire.words.len() <= crate::asr_hotwords::MAX_WORD_BYTES);
        assert!(wire
            .words
            .lines()
            .all(|word| names.iter().any(|name| word == name)));
        assert_eq!(
            crate::asr_hotwords::validate_words(&wire.words).unwrap(),
            wire.words
        );
        assert_eq!(terms.fingerprint(), Terms::new(&[], &[]).fingerprint());
    }
    #[test]
    fn note_names_handle_split_asr_spelling_without_consuming_other_words() {
        let terms = Terms::new(&[note("测试omascribe和PixelLedger的功能")], &[]);
        for (input, expected) in [
            ("这个 ALMA SCRIBE的功能", "这个 OmaScribe的功能"),
            ("I use OMA SCRIBE today", "I use OmaScribe today"),
            ("测试奥马 SCRIB的一些功能", "测试OmaScribe的一些功能"),
            ("左侧已经写了奥马斯", "左侧已经写了OmaScribe"),
            (
                "测试 PIXEL LEDGER 和 PIXEL LEDGRE",
                "测试 PixelLedger 和 PixelLedger",
            ),
            ("OMASCRIBE", "OmaScribe"),
        ] {
            assert_eq!(terms.normalize(input), expected, "{input}");
        }
        for input in [
            "OS, OAS, SCRIBE",
            "MYOMASCRIBER",
            "ALMOST RIB RIGHT NOW",
            "预算3000，奥斯的方案",
            "I transcribe interviews",
        ] {
            assert_eq!(terms.normalize(input), input);
        }
        assert_eq!(Terms::new(&[], &[]).normalize("ALMA SCRIBE"), "ALMA SCRIBE");
        assert_eq!(
            Terms::new(&[note("We need transcription improvements")], &[])
                .normalize("TRANSCRIPTON"),
            "TRANSCRIPTON"
        );
    }
    #[test]
    fn competing_names_are_not_merged() {
        let terms = Terms::new(&[note("测试OmaScribe和OpaScribe")], &[]);
        assert_eq!(terms.normalize("ONA SCRIBE"), "ONA SCRIBE");
        assert_eq!(terms.normalize("OpaScribe"), "OpaScribe");
        let terms = Terms::new(&[note("测试OmaScribe和AlmaScribe")], &[]);
        assert_eq!(terms.normalize("ALMA SCRIBE"), "AlmaScribe");
        let terms = Terms::new(
            &[note("测试OmaScribe")],
            &[Entry {
                kind: "term".into(),
                title: "Alma".into(),
                ..Default::default()
            }],
        );
        assert_eq!(terms.normalize("Alma SCRIBE"), "Alma SCRIBE");
        let terms = Terms::new(
            &[note("测试OmaScribe")],
            &[Entry {
                kind: "term".into(),
                title: "奥马斯".into(),
                ..Default::default()
            }],
        );
        assert_eq!(terms.normalize("奥马斯的方案"), "奥马斯的方案");
    }
    #[test]
    fn draft_normalization_keeps_source_ids_and_only_changes_prose() {
        let terms = Terms::new(&[note("测试omascribe")], &[]);
        let value: serde_json::Value = serde_json::from_str(&terms.normalize_draft(
            r#"{"sections":[{"title":"ALMA SCRIBE","body":"ALMA SCRIBE尚未完成。","sources":[1,3]}]}"#
        ).unwrap()).unwrap();
        assert_eq!(value["sections"][0]["body"], "OmaScribe尚未完成。");
        assert_eq!(value["sections"][0]["sources"], serde_json::json!([1, 3]));
    }
    #[test]
    fn automatic_edits_can_follow_new_aliases_but_human_edits_remain_locked() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let m = e.create_meeting("Term revisions").unwrap();
        let s = crate::Segment {
            id: format!("{}:1:1", m.id),
            meeting_id: m.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 3000,
            text: "ALMA SCRIBE尚未修复".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        e.store.lock().unwrap().save_segment(&s).unwrap();
        e.save_note(&m.id, None, "测试omascribe", "document", 0)
            .unwrap();
        assert_eq!(
            e.snapshot(&m.id).unwrap().segments[0].text,
            "OmaScribe尚未修复"
        );
        let mut term = e
            .save_library_entry(Entry {
                kind: "term".into(),
                title: "OmaScribe Studio".into(),
                aliases: "ALMA SCRIBE".into(),
                ..Default::default()
            })
            .unwrap();
        let revised = e.snapshot(&m.id).unwrap().segments[0].clone();
        assert_eq!(revised.text, "OmaScribe Studio尚未修复");
        assert_eq!(revised.revision, 3);
        e.edit_segment(&s.id, "手动确认原话为 ALMA SCRIBE", 3)
            .unwrap();
        term.title = "OmaScribe Desktop".into();
        e.save_library_entry(term).unwrap();
        assert_eq!(
            e.snapshot(&m.id).unwrap().segments[0].text,
            "手动确认原话为 ALMA SCRIBE"
        );
        let original: String = e
            .store
            .lock()
            .unwrap()
            .conn
            .query_row(
                "SELECT json FROM segment_history WHERE id=?1 AND revision=1",
                [&s.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<crate::Segment>(&original)
                .unwrap()
                .text,
            s.text
        );
    }
    #[test]
    fn implicit_date_focus_can_translate_the_actual_calendar_draft() {
        let notes = [note("**date**")];
        let source = "The meeting date is Tuesday, 2024-5-7.";
        let body = "会议日期是2024年5月7日星期二。";
        let terms = Terms::new(&notes, &[]);
        assert!(terms.validate_writer_names(source, body).is_ok());
        assert_eq!(notes[0].content, "**date**");
    }
    #[test]
    fn focused_chinese_honorifics_preserve_the_complete_spoken_spelling() {
        assert_ne!(
            Terms::new(&[note("**展览** **预算** **刘老师**")], &[]).fingerprint(),
            Terms::new(&[note("**展览** **预算**")], &[]).fingerprint()
        );
        for (content, name) in [
            ("**展览** **预算** **刘老师**", "刘老师"),
            ("布置由 **欧阳讲师** 跟进", "欧阳讲师"),
            ("- 赵教授", "赵教授"),
            ("# 李博士", "李博士"),
            ("**劉老師**", "劉老師"),
            ("**張講師**", "張講師"),
            ("**和老师**", "和老师"),
        ] {
            let terms = Terms::new(&[note(content)], &[]);
            let source = format!("{name}继续负责布置。");
            assert_eq!(terms.normalize(&source), source);
            assert_eq!(terms.spellings(&source), [name]);
            assert_eq!(terms.caption_spellings(&source), [name]);
            assert!(terms.hotwords.contains(&name.to_owned()));
            assert!(terms.validate_writer_names(&source, &source).is_ok());
            assert!(terms
                .validate_writer_names(
                    &source,
                    "Mr. Liu continues to be responsible for assigning."
                )
                .is_err());
        }
    }
    #[test]
    fn honorific_focus_does_not_inject_unspoken_names_or_freeze_generic_roles() {
        let terms = Terms::new(&[note("**刘老师**")], &[]);
        for source in [
            "王老师继续负责布置。",
            "刘教授继续负责布置。",
            "布置还没有完成。",
        ] {
            assert!(terms.spellings(source).is_empty(), "{source}");
            assert!(terms.caption_spellings(source).is_empty(), "{source}");
            assert_eq!(terms.normalize(source), source);
        }
        for content in [
            "老师",
            "**老师**",
            "**各位老师**",
            "**任课老师**",
            "**大学教授**",
            "**数学博士**",
            "**一位讲师**",
            "刘老师继续负责布置。",
            "**王或刘老师**",
        ] {
            let terms = Terms::new(&[note(content)], &[]);
            assert!(terms.hotwords.is_empty(), "{content}");
            assert!(terms.caption_spellings(content).is_empty(), "{content}");
        }
    }
    #[test]
    fn explicit_terms_override_implicit_honorific_hints_and_concept_filters() {
        let entries = [Entry {
            kind: "term".into(),
            title: "Teacher Liu".into(),
            aliases: "刘老师".into(),
            ..Default::default()
        }];
        let terms = Terms::new(&[note("**刘老师** **date** **test** **budget**")], &entries);
        assert_eq!(
            terms.normalize("刘老师继续负责布置。"),
            "Teacher Liu继续负责布置。"
        );
        assert_eq!(
            terms.caption_spellings("刘老师继续负责布置。"),
            ["Teacher Liu"]
        );
        assert!(terms
            .validate_writer_names(
                "刘老师继续负责布置。",
                "Teacher Liu continues to be responsible for assigning."
            )
            .is_ok());
        for word in ["date", "test", "budget"] {
            assert!(terms.hotwords.contains(&word.to_owned()));
            assert!(terms.caption_spellings(word).is_empty());
            let entries = [Entry {
                kind: "term".into(),
                title: word.into(),
                ..Default::default()
            }];
            let terms = Terms::new(&[note(&format!("**{word}** **刘老师**"))], &entries);
            assert_eq!(terms.caption_spellings(word), [word]);
            assert!(terms.validate_writer_names(word, "概念").is_err());
        }
    }
    #[test]
    fn implicit_concept_keywords_remain_asr_focus_without_literal_spelling_obligations() {
        for (word, source, body) in [
            (
                "date",
                "The meeting date is Tuesday, 2024-5-7.",
                "会议日期是2024年5月7日星期二。",
            ),
            ("Date", "The meeting date is Tuesday.", "会议日期是星期二。"),
            ("test", "The test is not complete.", "测试尚未完成。"),
            ("Test", "The test is not complete.", "测试尚未完成。"),
            (
                "budget",
                "The budget has not been approved.",
                "预算尚未批准。",
            ),
            (
                "Budget",
                "The budget has not been approved.",
                "预算尚未批准。",
            ),
        ] {
            let content = format!("**{word}**");
            let notes = [note(&content)];
            let terms = Terms::new(&notes, &[]);
            assert!(terms.hotwords.contains(&word.to_owned()), "{word}");
            assert!(terms.spellings(source).is_empty(), "{word}");
            assert!(terms.caption_spellings(source).is_empty(), "{word}");
            assert!(terms.validate_writer_names(source, body).is_ok(), "{word}");
            assert_eq!(notes[0].content, content);
        }
        let terms = Terms::new(&[note("关键词 **Budget** 和 **date**")], &[]);
        assert!(terms.hotwords.contains(&"Budget".to_owned()));
        assert!(terms
            .validate_writer_names("The budget date is pending.", "预算日期待定。")
            .is_ok());
    }
    #[test]
    fn literal_identifiers_names_and_explicit_terms_keep_exact_spelling_requirements() {
        for word in [
            "OmaScribe",
            "omascribe",
            "meetingDate",
            "date2024",
            "DATE",
            "CUDA",
            "Alice",
        ] {
            let terms = Terms::new(&[note(&format!("**{word}**"))], &[]);
            let source = format!("The {word} feature is not ready.");
            assert!(
                terms
                    .validate_writer_names(&source, "功能尚未就绪。")
                    .is_err(),
                "{word}"
            );
            let canonical = if word.eq_ignore_ascii_case("omascribe") {
                "OmaScribe"
            } else {
                word
            };
            assert!(
                terms
                    .validate_writer_names(&source, &format!("{canonical}功能尚未就绪。"))
                    .is_ok(),
                "{word}"
            );
            if word != "Alice" {
                assert!(
                    terms
                        .caption_spellings(&source)
                        .contains(&canonical.to_owned()),
                    "{word}"
                );
            }
        }
        let terms = Terms::new(&[note("**date**\n\n**林悦**")], &[]);
        let source = "离线转写工具由林悦负责测试。";
        assert!(terms
            .validate_writer_names(source, "离线转写工具由林悦负责测试。")
            .is_ok());
        assert!(terms
            .validate_writer_names(source, "离线转写工具需要测试。")
            .is_err());
        for word in ["date", "test", "budget"] {
            let mut explicit_note = note(&format!("**{word}**"));
            explicit_note.kind = "term".into();
            let from_note = Terms::new(&[explicit_note], &[]);
            let entries = [Entry {
                kind: "term".into(),
                title: word.into(),
                ..Default::default()
            }];
            let from_entry = Terms::new(&[note(&format!("**{word}**"))], &entries);
            let source = format!("The {word} feature is not ready.");
            for terms in [&from_note, &from_entry] {
                assert!(
                    terms
                        .validate_writer_names(&source, "功能尚未就绪。")
                        .is_err(),
                    "{word}"
                );
                assert!(
                    terms
                        .validate_writer_names(&source, &format!("{word}功能尚未就绪。"))
                        .is_ok(),
                    "{word}"
                );
                assert!(
                    terms.caption_spellings(&source).contains(&word.to_owned()),
                    "{word}"
                );
            }
        }
        let entries = [Entry {
            kind: "term".into(),
            title: "CalendarDate".into(),
            aliases: "date".into(),
            ..Default::default()
        }];
        let terms = Terms::new(&[note("**date**")], &entries);
        assert_eq!(terms.normalize("date"), "CalendarDate");
        assert!(terms
            .validate_writer_names("The date field is pending.", "日期字段待定。")
            .is_err());
        assert!(terms
            .validate_writer_names("The date field is pending.", "CalendarDate字段待定。")
            .is_ok());
    }
    #[test]
    fn writer_name_classification_changes_identity_without_changing_asr_words() {
        let mut terms = Terms::new(&[note("**date**")], &[]);
        let old_identity = terms.fingerprint();
        let hotwords = terms.hotwords.clone();
        let normalized = terms.normalize("The meeting date is Tuesday.");
        // The old classification imposed this obligation with the same ASR
        // mappings. Cache identity must distinguish that semantic contract.
        terms.writer_names.push("date".into());
        assert_ne!(terms.fingerprint(), old_identity);
        assert_eq!(terms.hotwords, hotwords);
        assert_eq!(terms.normalize("The meeting date is Tuesday."), normalized);
    }
    #[test]
    fn lowercase_keyword_lines_guide_names_without_inventing_missing_subjects() {
        let terms = Terms::new(&[note("omascribe\n\ngtx1650\n\nnvidia")], &[]);
        assert_eq!(terms.hotwords, ["OmaScribe", "gtx1650", "nvidia"]);
        assert!(Terms::new(&[note("we can review the budget")], &[])
            .hotwords
            .is_empty());
        assert!(terms
            .spellings("今天只讨论麦克风权限，预算未确定。")
            .is_empty());
        assert!(terms.spellings("MYOMASCRIBER").is_empty());
        assert!(terms
            .validate_writer_names("OmaScribe 尚未修复。", "功能尚未修复。")
            .is_err());
        assert!(terms
            .validate_writer_names("OmaScribe 尚未修复。", "OmaScribe 功能尚未修复。")
            .is_ok());
        assert!(terms
            .validate_writer_names("M二的功能尚未运行。", "功能尚未运行。")
            .is_ok());
    }
    #[test]
    fn mixed_language_asr_words_may_translate_without_becoming_required_names() {
        let terms = Terms::new(
            &[note(
                "**录音** **offline transcription** **系统音频** OmaScribe",
            )],
            &[],
        );
        let source = "OmaScribe offline transcription is ready for review.";
        assert!(terms.hotwords.contains(&"offline".to_owned()));
        assert_eq!(terms.spellings(source), ["OmaScribe"]);
        assert!(terms
            .validate_writer_names(source, "OmaScribe 离线转写原型已可评审。")
            .is_ok());
        assert!(terms
            .validate_writer_names(source, "离线转写原型已可评审。")
            .is_err());
    }

    #[test]
    fn ordinary_mixed_language_focus_preserves_a_spoken_plural() {
        for (word, source) in [
            ("transcription", "The transcriptions are incomplete."),
            ("recording", "The recordings are incomplete."),
        ] {
            let terms = Terms::new(&[note(&format!("录音 {word}"))], &[]);
            assert!(terms.recognition_hotwords().contains(&word.to_owned()));
            assert!(terms.spellings(source).is_empty());
            assert_eq!(terms.normalize(source), source);
            assert_eq!(terms.normalize(&word.to_uppercase()), word);
        }
        let glossary = Terms::new(
            &[],
            &[Entry {
                kind: "term".into(),
                title: "PixelLedger".into(),
                ..Default::default()
            }],
        );
        assert_eq!(
            glossary.normalize("PIXEL LEDGRE remains untested."),
            "PixelLedger remains untested."
        );
    }

    fn glossary(count: usize) -> Vec<Entry> {
        (0..count)
            .map(|i| Entry {
                kind: "term".into(),
                title: format!("AName{i:03}"),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn active_note_name_precedes_eighty_unreferenced_global_names() {
        let entries = glossary(80);
        let plain = Terms::new(&[], &entries);
        let old_order: Vec<_> = entries.iter().map(|entry| entry.title.clone()).collect();
        assert_eq!(plain.hotwords, old_order);
        assert_eq!(plain.recognition_hotwords(), old_order);
        let terms = Terms::new(&[note("ZoraLedger")], &entries);
        let hints = terms.recognition_hotwords();
        assert_eq!(hints.len(), 80);
        assert_eq!(hints[0], "ZoraLedger");
        assert_eq!(&hints[1..], &old_order[..79]);
        assert_eq!(terms.writer_names.len(), 80);
        assert_eq!(terms.caption_names.len(), 80);
        assert_eq!(
            terms.spellings("ZoraLedger remains untested."),
            ["ZoraLedger"]
        );
        assert_eq!(
            terms.caption_spellings("ZoraLedger remains untested."),
            ["ZoraLedger"]
        );
    }

    #[test]
    fn active_note_selects_a_global_canonical_or_alias_before_the_entry_cap() {
        for (canonical, aliases) in [("ZoraLedger", ""), ("Zora-Ledger", "ZoraLedger")] {
            let mut entries = glossary(80);
            entries.push(Entry {
                kind: "term".into(),
                title: canonical.into(),
                aliases: aliases.into(),
                ..Default::default()
            });
            let terms = Terms::new(&[note("ZoraLedger")], &entries);
            let hints = terms.recognition_hotwords();
            assert_eq!(hints.len(), 80);
            assert_eq!(hints[0], "ZoraLedger");
            assert_eq!(
                terms.normalize("ZoraLedger remains untested."),
                format!("{canonical} remains untested.")
            );
            assert_eq!(terms.spellings("ZoraLedger remains untested."), [canonical]);
            assert_eq!(
                terms.caption_spellings("ZoraLedger remains untested."),
                [canonical]
            );
            assert_eq!(
                Terms::new(&[], &entries).hotwords,
                glossary(80)
                    .into_iter()
                    .map(|entry| entry.title)
                    .collect::<Vec<_>>()
            );
        }
        let mut entries = glossary(80);
        for title in ["Zora-Ledger", "Other-Ledger"] {
            entries.push(Entry {
                kind: "term".into(),
                title: title.into(),
                aliases: "SharedName".into(),
                ..Default::default()
            });
        }
        let conflicts = Terms::new(&[note("SharedName")], &entries);
        assert!(!conflicts
            .recognition_hotwords()
            .iter()
            .any(|word| word.eq_ignore_ascii_case("SharedName")));
        assert_eq!(
            conflicts.normalize("SharedName remains untested."),
            "SharedName remains untested."
        );
    }

    #[test]
    fn active_note_priority_also_preserves_near_name_candidates() {
        let entries: Vec<_> = (0..80)
            .map(|i| Entry {
                kind: "term".into(),
                title: format!(
                    "AlphaName{}{}",
                    char::from(b'a' + i / 26),
                    char::from(b'a' + i % 26)
                ),
                ..Default::default()
            })
            .collect();
        let terms = Terms::new(&[note("ZoraLedger\n\n录音 transcription")], &entries);
        assert_eq!(terms.approximate.len(), 80);
        assert_eq!(terms.approximate[0], "ZoraLedger");
        assert_eq!(
            terms.normalize("ZoraLedgr remains untested."),
            "ZoraLedger remains untested."
        );
        let ordinary = "Only transcriptions remain incomplete.";
        assert_eq!(terms.normalize(ordinary), ordinary);
        assert!(terms.spellings(ordinary).is_empty());
        assert!(!terms.approximate.contains(&"transcription".to_owned()));
    }

    #[test]
    fn priority_uses_only_existing_admitted_note_name_classes() {
        let entries = glossary(80);
        for (content, hint, canonical) in [
            ("**ZoraLedger**", "ZoraLedger", "ZoraLedger"),
            ("**王老师**", "王老师", "王老师"),
            ("gtx1650", "gtx 一六五零", "gtx1650"),
        ] {
            let terms = Terms::new(&[note(content)], &entries);
            assert_eq!(terms.recognition_hotwords()[0], hint);
            assert!(terms.writer_names.contains(&canonical.to_owned()));
            assert!(terms.caption_names.contains(&canonical.to_owned()));
        }
        let mut explicit_note = note("录音 terminology");
        explicit_note.kind = "term".into();
        let terms = Terms::new(&[explicit_note], &entries);
        assert_eq!(terms.recognition_hotwords()[0], "terminology");
        assert!(terms.writer_names.contains(&"terminology".to_owned()));
        let plain = Terms::new(&[], &entries);
        assert_eq!(
            Terms::new(&[note("**张敬先**")], &entries).recognition_hotwords(),
            plain.recognition_hotwords()
        );
    }

    #[test]
    fn active_punctuation_alias_inherits_canonical_priority_and_glossary_spelling() {
        let mut entries = vec![Entry {
            kind: "term".into(),
            title: "Node.js".into(),
            aliases: "NODE JS,SHARED".into(),
            ..Default::default()
        }];
        entries.extend(glossary(79));
        let terms = Terms::new(&[note("Node.js\n\n录音 transcription")], &entries);
        let hints = terms.recognition_hotwords();
        assert_eq!(hints.len(), 80);
        assert_eq!(&hints[..2], ["NODE JS", "SHARED"]);
        assert!(!hints.contains(&"transcription".to_owned()));
        assert_eq!(
            terms.normalize("NODE JS remains untested."),
            "Node.js remains untested."
        );
        let mut entries = glossary(79);
        entries.push(Entry {
            kind: "term".into(),
            title: "CanonicalNode".into(),
            aliases: "ZoraLedger".into(),
            ..Default::default()
        });
        let explicit = Terms::new(&[note("ZoraLedger")], &entries);
        assert_eq!(explicit.recognition_hotwords()[0], "CanonicalNode");
        assert_eq!(
            explicit.normalize("ZoraLedger remains untested."),
            "CanonicalNode remains untested."
        );
        let conflicts = Terms::new(
            &[note("SHARED")],
            &[
                Entry {
                    kind: "term".into(),
                    title: "C++".into(),
                    aliases: "SHARED".into(),
                    ..Default::default()
                },
                Entry {
                    kind: "term".into(),
                    title: "C--".into(),
                    aliases: "SHARED".into(),
                    ..Default::default()
                },
            ],
        );
        assert!(conflicts.recognition_hotwords().is_empty());
        assert_eq!(
            conflicts.normalize("SHARED remains untested."),
            "SHARED remains untested."
        );
    }

    #[test]
    fn generic_mixed_prose_never_displaces_explicit_global_names() {
        let mut entries = glossary(80);
        for entry in &mut entries {
            entry.title = entry.title.to_lowercase();
        }
        let plain = Terms::new(&[], &entries);
        let terms = Terms::new(&[note("录音 budget transcription recordings")], &entries);
        assert_eq!(terms.recognition_hotwords(), plain.recognition_hotwords());
        assert_eq!(terms.hotwords.len(), 80);
    }

    #[test]
    fn admitted_note_overflow_and_case_duplicates_use_one_deterministic_hint_budget() {
        let names: Vec<_> = (0..81).map(|i| format!("NoteName{i:03}")).collect();
        let notes = [note(&names.join("\n"))];
        let terms = Terms::new(&notes, &[]);
        assert_eq!(terms.recognition_hotwords(), names[..80]);
        assert_eq!(
            Terms::new(&notes, &[]).recognition_hotwords(),
            terms.recognition_hotwords()
        );
        let mut entries = glossary(79);
        entries.push(Entry {
            kind: "term".into(),
            title: "ZoraLedger".into(),
            ..Default::default()
        });
        let duplicate = Terms::new(&[note("ZoraLedger\nZORALEDGER\nzoraledger")], &entries);
        let hints = duplicate.recognition_hotwords();
        assert_eq!(hints.len(), 80);
        assert_eq!(hints[0], "ZoraLedger");
        assert_eq!(
            hints
                .iter()
                .filter(|word| word.eq_ignore_ascii_case("zoraledger"))
                .count(),
            1
        );
        assert_eq!(
            crate::asr_hotwords::encode_words(&hints)
                .stats
                .accepted_terms,
            80
        );
    }
    #[test]
    fn writer_receives_note_spelling_without_opt_in_knowledge_or_memory() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let context = e
            .writer_context(&[note("测试omascribe的功能 &nbsp;")], "ALMA SCRIBE 的功能")
            .unwrap();
        assert_eq!(context["spellings"], serde_json::json!(["OmaScribe"]));
        assert_eq!(context["background_notes"], serde_json::json!([]));
        assert_eq!(context["writing_preferences"], serde_json::json!([]));
        let english = e
            .writer_context(&[note("Review PixelLedger.")], "PIXEL LEDGRE fails")
            .unwrap();
        assert_eq!(english["spellings"], serde_json::json!(["PixelLedger"]));
        assert_eq!(
            Terms::new(&[note("Review PixelLedger")], &[]).normalize("under review"),
            "under review"
        );
    }
    #[test]
    fn aliases_are_bounded_and_require_explicit_context() {
        let entries = vec![Entry {
            kind: "term".into(),
            title: "OmaScribe".into(),
            aliases: "奥马（OMA）的SCRIB,奥马的SCRIB".into(),
            ..Default::default()
        }];
        let t = Terms::new(&[], &entries);
        assert_eq!(
            t.normalize("测试奥马（OMA）的SCRIB，OMASCRIBE未完成。"),
            "测试OmaScribe，OmaScribe未完成。"
        );
        assert_eq!(
            t.normalize("MYOMASCRIBER OS 预算3000"),
            "MYOMASCRIBER OS 预算3000"
        );
        assert_eq!(Terms::new(&[], &[]).normalize("奥马的SCRIB"), "奥马的SCRIB");
    }

    #[test]
    fn implicit_hyphen_focus_remains_translatable_in_minutes() {
        let terms = Terms::new(&[note("follow-up")], &[]);
        let source = "A follow-up is still pending.";
        assert!(terms.spellings(source).is_empty());
        assert!(terms
            .validate_writer_names(source, "跟进仍待处理。")
            .is_ok());
        let mut declared = note("follow-up");
        declared.kind = "term".into();
        assert_eq!(
            Terms::new(&[declared], &[]).spellings(source),
            ["follow-up"]
        );
        for (title, alias) in [("C--", "C MINUS MINUS"), ("follow-up", "FOLLOW UP")] {
            let explicit = Terms::new(
                &[],
                &[Entry {
                    kind: "term".into(),
                    title: title.into(),
                    aliases: alias.into(),
                    ..Default::default()
                }],
            );
            assert_eq!(
                explicit.spellings(&format!("{alias} remains pending.")),
                [title]
            );
        }
        let identifiers = Terms::new(&[note("C++\nNode.js\nGPT-4")], &[]);
        for name in ["C++", "Node.js", "GPT-4"] {
            assert_eq!(
                identifiers.spellings(&format!("{name} remains pending.")),
                [name]
            );
        }
    }

    #[test]
    fn standalone_note_punctuation_names_keep_literal_caption_spellings() {
        let terms = Terms::new(&[note("C++\nNode.js")], &[]);
        let source = "C++ and Node.js remain untested.";
        assert_eq!(terms.spellings(source), ["C++", "Node.js"]);
        assert_eq!(terms.caption_spellings(source), ["C++", "Node.js"]);
        assert_eq!(terms.normalize(source), source);
        assert!(terms.recognition_hotwords().is_empty());
        assert_eq!(
            terms.normalize("C PLUS PLUS and NODE JS"),
            "C PLUS PLUS and NODE JS"
        );
        assert!(terms
            .caption_spellings("The test remains pending.")
            .is_empty());
        assert!(terms.spellings("The test remains pending.").is_empty());
        let generic = Terms::new(&[note("录音 note-taking\nfollow-up")], &[]);
        assert!(generic
            .caption_spellings("The note-taking follow-up remains pending.")
            .is_empty());
    }

    #[test]
    fn punctuation_names_preserve_display_spellings_and_only_explicit_decoder_aliases() {
        let entries: Vec<_> = [
            ("C++", "C PLUS PLUS"),
            ("GPT-4", "GPT FOUR"),
            ("Node.js", "NODE JS"),
        ]
        .into_iter()
        .map(|(title, aliases)| Entry {
            kind: "term".into(),
            title: title.into(),
            aliases: aliases.into(),
            ..Default::default()
        })
        .collect();
        let terms = Terms::new(&[], &entries);
        let source = "C PLUS PLUS and GPT FOUR use NODE JS. The test is still pending.";
        let corrected = "C++ and GPT-4 use Node.js. The test is still pending.";
        assert_eq!(terms.normalize(source), corrected);
        assert!(terms.hotwords.is_empty());
        assert_eq!(
            terms.recognition_hotwords(),
            ["C PLUS PLUS", "GPT FOUR", "NODE JS"]
        );
        let wire = crate::asr_hotwords::encode_words(&terms.recognition_hotwords());
        assert_eq!(wire.words, "C PLUS PLUS\nGPT FOUR\nNODE JS");
        assert_eq!(
            crate::asr_hotwords::validate_words(&wire.words).unwrap(),
            wire.words
        );
        assert_eq!(terms.spellings(source), ["C++", "GPT-4", "Node.js"]);
        assert_eq!(terms.caption_spellings(source), ["C++", "GPT-4", "Node.js"]);
        assert!(terms.validate_writer_names(source, corrected).is_ok());
        assert!(terms
            .validate_writer_names(source, "The test is still pending.")
            .is_err());
        assert!(terms.spellings("The test is still pending.").is_empty());
        assert!(terms
            .validate_writer_names("The test is still pending.", "The test is still pending.")
            .is_ok());
        // Knowing punctuation alone never invents a pronunciation or an alias.
        let unaliased = Terms::new(
            &[],
            &[Entry {
                kind: "term".into(),
                title: "Node.js".into(),
                ..Default::default()
            }],
        );
        assert!(unaliased.recognition_hotwords().is_empty());
        assert_eq!(unaliased.normalize("NODE JS"), "NODE JS");
        assert_eq!(
            unaliased.spellings("Node.js remains untested."),
            ["Node.js"]
        );
    }

    #[test]
    fn punctuation_aliases_keep_native_syntax_conflicts_and_disabled_entries_out_of_hints() {
        let entries: Vec<_> = [
            (
                "C++",
                "SHARED,C PLUS PLUS,C PLUS PLUS :99999,C\tPLUS,C\0PLUS",
                true,
                false,
            ),
            ("C--", "SHARED,C MINUS MINUS", true, false),
            ("GPT-4", "DISABLED", false, false),
            ("Node.js", "DELETED", true, true),
            ("BAD\0TITLE", "CONTROL TITLE", true, false),
        ]
        .into_iter()
        .map(|(title, aliases, enabled, deleted)| Entry {
            kind: "term".into(),
            title: title.into(),
            aliases: aliases.into(),
            enabled,
            deleted,
            ..Default::default()
        })
        .collect();
        let terms = Terms::new(&[], &entries);
        assert_eq!(
            terms.normalize("SHARED DISABLED DELETED"),
            "SHARED DISABLED DELETED"
        );
        let hints = terms.recognition_hotwords();
        assert_eq!(hints, ["C PLUS PLUS", "C MINUS MINUS"]);
        let wire = crate::asr_hotwords::encode_words(&hints);
        assert_eq!(wire.words, "C PLUS PLUS\nC MINUS MINUS");
        assert_eq!(
            crate::asr_hotwords::validate_words(&wire.words).unwrap(),
            wire.words
        );
        assert!(terms.spellings("DISABLED DELETED CONTROL TITLE").is_empty());
    }

    #[test]
    fn punctuation_aliases_share_the_existing_recognition_and_spelling_bounds() {
        let terms = Terms::new(
            &[],
            &[Entry {
                kind: "term".into(),
                title: "Node.js".into(),
                aliases: (0..100)
                    .map(|i| format!("NODE JS {i}"))
                    .collect::<Vec<_>>()
                    .join(","),
                ..Default::default()
            }],
        );
        let hints = terms.recognition_hotwords();
        assert_eq!(hints.len(), crate::asr_hotwords::MAX_WORDS);
        let wire = crate::asr_hotwords::encode_words(&hints);
        assert_eq!(wire.stats.accepted_terms, crate::asr_hotwords::MAX_WORDS);
        assert!(wire.words.len() <= crate::asr_hotwords::MAX_WORD_BYTES);
        assert_eq!(
            crate::asr_hotwords::validate_words(&wire.words).unwrap(),
            wire.words
        );
        let entries: Vec<_> = (0..100)
            .map(|i| Entry {
                kind: "term".into(),
                title: format!("Name-{i}"),
                ..Default::default()
            })
            .collect();
        let many = Terms::new(&[], &entries);
        assert_eq!(many.writer_names.len(), 80);
        assert_eq!(many.caption_names.len(), 80);
    }

    #[test]
    fn saving_a_note_replays_punctuation_aliases_with_revised_sources_and_citations() -> Result<()>
    {
        let dir = tempfile::tempdir()?;
        let engine = Engine::open(dir.path().into())?;
        let meeting = engine.create_meeting("Punctuation spelling replay")?;
        engine.update_meeting(&meeting.id, &meeting.title, false)?;
        engine.save_library_entry(Entry {
            kind: "term".into(),
            title: "GPT-4".into(),
            aliases: "GPT FOUR".into(),
            ..Default::default()
        })?;
        let note =
            engine.save_note(&meeting.id, None, "Review the pending test.", "document", 0)?;
        let source = crate::Segment {
            id: format!("{}:1:1", meeting.id),
            meeting_id: meeting.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "GPT FOUR has not completed the test.".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        engine.store.lock().unwrap().save_segment(&source)?;
        let before = engine.snapshot(&meeting.id)?;
        engine.store.lock().unwrap().commit_document(
            &before,
            std::slice::from_ref(&source),
            1,
            vec![crate::Suggestion {
                id: String::new(),
                meeting_id: meeting.id.clone(),
                kind: "section".into(),
                focus: String::new(),
                text: source.text.clone(),
                status: "pending".into(),
                stale: false,
                sources: vec![crate::Source {
                    segment_id: source.id.clone(),
                    revision: source.revision,
                    start_ms: source.start_ms,
                    end_ms: source.end_ms,
                    quote: source.text.clone(),
                    speaker_name: None,
                }],
            }],
            false,
        )?;
        engine.save_note(
            &meeting.id,
            Some(note.id),
            "Review GPT-4's pending test.",
            "document",
            0,
        )?;
        let after = engine.snapshot(&meeting.id)?;
        let corrected = "GPT-4 has not completed the test.";
        assert_eq!(after.segments[0].text, corrected);
        assert_eq!(after.segments[0].revision, 2);
        assert_eq!(after.suggestions[0].text, corrected);
        assert!(!after.suggestions[0].stale);
        assert_eq!(after.suggestions[0].sources[0].quote, corrected);
        assert_eq!(after.suggestions[0].sources[0].revision, 2);
        let original: String = engine.store.lock().unwrap().conn.query_row(
            "SELECT json FROM segment_history WHERE id=?1 AND revision=1",
            [&source.id],
            |r| r.get(0),
        )?;
        assert_eq!(
            serde_json::from_str::<crate::Segment>(&original)?.text,
            source.text
        );
        engine.edit_segment(&source.id, "Human review keeps GPT FOUR as an example.", 2)?;
        assert_eq!(engine.correct_terminology(&meeting.id)?, 0);
        assert_eq!(
            engine.snapshot(&meeting.id)?.segments[0].text,
            "Human review keeps GPT FOUR as an example."
        );
        Ok(())
    }

    #[test]
    fn conflicting_aliases_are_not_guessed() {
        let entries = vec![
            Entry {
                kind: "term".into(),
                title: "Alpha".into(),
                aliases: "项目".into(),
                ..Default::default()
            },
            Entry {
                kind: "term".into(),
                title: "Beta".into(),
                aliases: "项目".into(),
                ..Default::default()
            },
        ];
        assert_eq!(
            Terms::new(&[], &entries).normalize("项目没有完成"),
            "项目没有完成"
        );
    }
    #[test]
    fn ambiguous_aliases_are_not_reenabled_by_near_spelling_or_concatenation() {
        for alias in ["CedarLedge", "Cedar Ledge", "Cedar-Ledge"] {
            let entries = vec![
                Entry {
                    kind: "term".into(),
                    title: "CedarLedger".into(),
                    aliases: alias.into(),
                    ..Default::default()
                },
                Entry {
                    kind: "term".into(),
                    title: "NimbusArchive".into(),
                    aliases: alias.to_ascii_uppercase(),
                    ..Default::default()
                },
            ];
            let terms = Terms::new(&[note("CedarLedgePilot")], &entries);
            for spelling in [
                "CedarLedge",
                "cedarledge",
                "CEDAR LEDGE",
                "CEDAR-LEDGE",
                "Cedar Ledge Pilot",
            ] {
                let source = format!("{spelling} remains untested.");
                assert_eq!(terms.normalize(&source), source, "alias={alias}");
                assert_eq!(terms.normalize_near_names(&source), source, "alias={alias}");
                let draft = serde_json::json!({"sections":[{"body":source}]}).to_string();
                let normalized: serde_json::Value =
                    serde_json::from_str(&terms.normalize_draft(&draft).unwrap()).unwrap();
                assert_eq!(normalized["sections"][0]["body"], source, "alias={alias}");
            }
            assert_eq!(
                terms.normalize("CEDARLEDGER remains untested."),
                "CedarLedger remains untested."
            );
            assert_eq!(
                terms.normalize("CedarLedgr remains untested."),
                "CedarLedger remains untested."
            );
        }
    }

    #[test]
    fn unicode_casefolded_ambiguous_alias_is_opaque_to_ascii_near_matching() {
        let entries: Vec<_> = ["Cribelyx", "NimbusArchive"]
            .into_iter()
            .map(|title| Entry {
                kind: "term".into(),
                title: title.into(),
                aliases: "Scribely".into(),
                ..Default::default()
            })
            .collect();
        let terms = Terms::new(&[], &entries);
        // Unicode case folding matches long s to S; lowercasing does not.
        let source = "ſcribely remains untested.";
        assert_eq!(terms.normalize(source), source);
        assert_eq!(terms.normalize_near_names(source), source);
        assert_eq!(
            terms.normalize("Cribely is pending."),
            "Cribelyx is pending."
        );
    }

    #[test]
    fn ambiguous_canonical_and_chinese_alias_spans_protect_their_full_spelling() {
        let terms = Terms::new(
            &[note("CedarLedger")],
            &[
                Entry {
                    kind: "term".into(),
                    title: "CedarLedge".into(),
                    aliases: "松账本".into(),
                    ..Default::default()
                },
                Entry {
                    kind: "term".into(),
                    title: "NimbusArchive".into(),
                    aliases: "CedarLedge,松账本".into(),
                    ..Default::default()
                },
                Entry {
                    kind: "term".into(),
                    title: "Delta".into(),
                    aliases: "松账".into(),
                    ..Default::default()
                },
            ],
        );
        for source in ["CedarLedge remains untested.", "松账本尚未完成。"] {
            assert_eq!(terms.normalize_exact(source), source);
            assert_eq!(terms.normalize(source), source);
        }
        assert_eq!(terms.normalize("松账尚未完成。"), "Delta尚未完成。");
        assert_eq!(
            terms.normalize("CEDARLEDGER remains untested."),
            "CedarLedger remains untested."
        );
    }

    #[test]
    fn ambiguous_alias_fingerprint_tracks_conflicts_without_blocking_duplicate_canonical() {
        let first = Entry {
            kind: "term".into(),
            title: "CedarLedger".into(),
            aliases: "CedarLedge".into(),
            ..Default::default()
        };
        let second = Entry {
            kind: "term".into(),
            title: "NimbusArchive".into(),
            aliases: "CEDARLEDGE".into(),
            ..Default::default()
        };
        let conflicting = Terms::new(&[], &[first.clone(), second.clone()]);
        let reversed = Terms::new(&[], &[second.clone(), first.clone()]);
        let without = Terms::new(
            &[],
            &[
                Entry {
                    aliases: String::new(),
                    ..first.clone()
                },
                Entry {
                    aliases: String::new(),
                    ..second
                },
            ],
        );
        assert_ne!(conflicting.fingerprint(), without.fingerprint());
        assert_eq!(conflicting.fingerprint(), reversed.fingerprint());
        let duplicates = Terms::new(&[], &[first.clone(), first]);
        assert_eq!(
            duplicates.normalize("CEDARLEDGE remains untested."),
            "CedarLedger remains untested."
        );
    }

    #[test]
    fn ambiguous_alias_outside_the_candidate_cap_still_revokes_selected_ownership() {
        let mut entries = vec![Entry {
            kind: "term".into(),
            title: "CedarLedger".into(),
            aliases: "CedarLedge".into(),
            ..Default::default()
        }];
        entries.extend(glossary(79));
        entries.push(Entry {
            kind: "term".into(),
            title: "NimbusArchive".into(),
            aliases: "CEDARLEDGE".into(),
            ..Default::default()
        });
        for notes in [vec![], vec![note("CedarLedge")], vec![note("CedarLedger")]] {
            let terms = Terms::new(&notes, &entries);
            assert_eq!(terms.hotwords.len(), 80);
            for source in [
                "CedarLedge remains untested.",
                "CEDAR LEDGE remains untested.",
            ] {
                assert_eq!(terms.normalize(source), source);
                assert!(terms.spellings(source).is_empty());
            }
            assert_eq!(
                terms.normalize("CEDARLEDGER remains untested."),
                "CedarLedger remains untested."
            );
        }
    }

    #[test]
    fn ambiguous_model_alias_outside_the_cap_cannot_revive_through_derived_spellings() {
        let mut entries = vec![Entry {
            kind: "term".into(),
            title: "CanonicalA".into(),
            aliases: "MX2750".into(),
            ..Default::default()
        }];
        entries.extend(glossary(79));
        entries.push(Entry {
            kind: "term".into(),
            title: "CanonicalB".into(),
            aliases: "MX2750".into(),
            ..Default::default()
        });
        let terms = Terms::new(&[], &entries);
        for spelling in model_spellings("MX2750").unwrap() {
            let source = format!("{spelling} remains untested.");
            assert_eq!(terms.normalize(&source), source);
        }
        assert!(!terms
            .recognition_hotwords()
            .iter()
            .any(|hint| hint.to_ascii_uppercase().starts_with("MX")));
        assert_eq!(
            terms.normalize("CanonicalA remains untested."),
            "CanonicalA remains untested."
        );
    }
    #[test]
    fn note_spelling_corrects_transcript_but_keeps_raw_revision_and_human_edits() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let m = e.create_meeting("Term test").unwrap();
        let segment = crate::Segment {
            id: format!("{}:1:1", m.id),
            meeting_id: m.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 3000,
            text: "正在测试奥马（OMA）的SCRIB，尚未完成。".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        e.store.lock().unwrap().save_segment(&segment).unwrap();
        e.save_note(&m.id, None, "测试omascribe的功能", "document", 0)
            .unwrap();
        let corrected = e.snapshot(&m.id).unwrap().segments[0].clone();
        assert_eq!(corrected.text, "正在测试OmaScribe，尚未完成。");
        assert_eq!(corrected.revision, 2);
        let raw: String = e
            .store
            .lock()
            .unwrap()
            .conn
            .query_row(
                "SELECT json FROM segment_history WHERE id=?1 AND revision=1",
                [&segment.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<crate::Segment>(&raw).unwrap().text,
            segment.text
        );
        e.edit_segment(&segment.id, "人工确认：奥马的SCRIB是错误示例", 2)
            .unwrap();
        assert_eq!(e.correct_terminology(&m.id).unwrap(), 0);
        assert!(e.snapshot(&m.id).unwrap().segments[0]
            .text
            .contains("错误示例"));
    }
}
