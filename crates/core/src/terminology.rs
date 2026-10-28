//! Local spelling context: explicit aliases and unambiguous near-spellings of long names.
use crate::{library::Entry, Engine, UserNote};
use anyhow::Result;
use regex::{Regex, RegexBuilder};
use rusqlite::OptionalExtension;
use std::collections::{BTreeMap, HashSet};

pub struct Terms {
    matcher: Option<Regex>,
    names: BTreeMap<String, String>,
    model_aliases: HashSet<String>,
    pub hotwords: Vec<String>,
    writer_names: Vec<String>,
    caption_names: Vec<String>,
    actor_spellings: Vec<String>,
    approximate: Vec<String>,
}

// A complete explicit graphics-series keyword may supply a model number.
// Monetary abbreviations such as "RMB 3000" are not graphics identifiers.
fn model_identifier(value: &str) -> Option<(&str, &str)> {
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

fn model_continuation(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || c.is_numeric()
        || c == '_'
        || "零〇一二三四五六七八九十百千万亿两幺点%％".contains(c)
}

fn model_has_continuation(text: &str) -> bool {
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
    if [
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
    ]
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
impl Terms {
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut model_aliases: Vec<_> = self.model_aliases.iter().collect();
        model_aliases.sort_unstable();
        let payload = if model_aliases.is_empty() {
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
        let mut writer_names = Vec::new();
        let mut caption_names = Vec::new();
        let mut approximate = Vec::new();
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
        for e in entries
            .iter()
            .filter(|e| e.kind == "term" && e.enabled && !e.deleted)
            .take(80)
        {
            hotwords.push(e.title.trim().to_owned());
            writer_names.push(e.title.trim().to_owned());
            caption_names.push(e.title.trim().to_owned());
            approximate.push(e.title.trim().to_owned());
            add(&e.title, &e.title);
            for alias in std::iter::once(e.title.as_str())
                .chain(e.aliases.split([',', '，', ';', '；', '\n', '|']))
            {
                add(alias, &e.title);
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
        // User-written Latin names supply spelling/casing, never facts or inferred aliases.
        let latin = Regex::new(r"[A-Za-z][A-Za-z0-9_+.-]{2,39}").unwrap();
        for n in crate::notes::document_notes(notes) {
            for line in n.content.lines() {
                let name = line.trim().trim_matches(['*', '-', '#', ' ']);
                let Some((prefix, digits)) = model_identifier(name) else {
                    continue;
                };
                let compact = format!("{prefix}{digits}");
                let spaced = format!("{prefix} {digits}");
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
            let chinese_context = n
                .content
                .chars()
                .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
            let mut accepted_words = HashSet::new();
            for word in latin.find_iter(&n.content) {
                let line_start = n.content[..word.start()].rfind('\n').map_or(0, |i| i + 1);
                let line_end = n.content[word.end()..]
                    .find('\n')
                    .map_or(n.content.len(), |i| word.end() + i);
                let line = n.content[line_start..line_end]
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
                // A standalone keyword line may supply a spelling even in
                // lowercase (gtx1650/nvidia); concept headings remain separate.
                let keyword_line = n
                    .content
                    .lines()
                    .any(|line| line.trim().trim_matches(['*', '-', '#', ' ']) == word);
                let identifier = word.chars().skip(1).any(|c| c.is_ascii_uppercase())
                    || word.chars().any(|c| c.is_ascii_digit())
                    || word.eq_ignore_ascii_case("omascribe");
                // A few ordinary concept headings are useful focus/ASR words,
                // not literal names. Explicit terms and strong identifiers
                // retain their spelling rules, including DATE as an acronym.
                let concept = n.kind != "term"
                    && !identifier
                    && ["date", "test", "budget"].contains(&word.to_ascii_lowercase().as_str());
                let named = identifier
                    || !concept
                        && (keyword_line
                            || word.starts_with(char::is_uppercase)
                                && n.content.contains(&format!("**{word}**")));
                let usable_asr_word =
                    n.kind == "term" || chinese_context || named || concept && keyword_line;
                if !usable_asr_word {
                    continue;
                }
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
                    }
                    // A generic keyword such as Budget is still translatable.
                    // Strong product identifiers and explicitly stored terms
                    // supply caption spellings, never additional speech facts.
                    if name.chars().skip(1).any(|c| c.is_ascii_uppercase())
                        || name.chars().any(|c| c.is_ascii_digit())
                        || n.kind == "term"
                    {
                        caption_names.push(name.to_owned());
                    }
                    approximate.push(name.to_owned());
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
        for key in conflicts {
            names.remove(&key);
        }
        model_aliases.retain(|key| names.contains_key(key));
        let mut aliases: Vec<_> = names.keys().collect();
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
        hotwords.retain(|s| {
            !s.is_empty()
                && s.chars().count() <= 80
                && s.chars().all(|c| c.is_alphanumeric() || c == ' ')
        });
        hotwords.sort();
        hotwords.dedup();
        hotwords.truncate(80);
        writer_names.retain(|s| hotwords.contains(s) && names.get(&s.to_lowercase()) == Some(s));
        writer_names.sort();
        writer_names.dedup();
        caption_names.retain(|s| hotwords.contains(s) && names.get(&s.to_lowercase()) == Some(s));
        caption_names.sort();
        caption_names.dedup();
        approximate.retain(|s| {
            (8..=40).contains(&s.len())
                && s.chars().all(|c| c.is_ascii_alphabetic())
                && names.get(&s.to_lowercase()) == Some(s)
        });
        approximate.sort();
        approximate.dedup();
        approximate.truncate(80);
        Self {
            matcher,
            names,
            model_aliases,
            hotwords,
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
            cursor = hit.end();
        }
        result.push_str(&text[cursor..]);
        result
    }

    /// The bundled token inventory represents spoken digits as CJK characters.
    /// Keep the display spelling for notes, but send a complete encodable hint.
    pub fn recognition_hotwords(&self) -> Vec<String> {
        let mut primary = Vec::new();
        let mut other_models = Vec::new();
        let mut alternatives = Vec::new();
        let mut title_fallbacks = Vec::new();
        for name in &self.hotwords {
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
        let mut seen = HashSet::new();
        primary
            .into_iter()
            .chain(other_models)
            .chain(title_fallbacks)
            .chain(alternatives)
            .filter(|word| seen.insert(word.to_lowercase()))
            .take(80)
            .collect()
    }
    fn normalize_near_names(&self, text: &str) -> String {
        if self.approximate.is_empty() {
            return text.to_owned();
        }
        static WORD: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let words: Vec<_> = WORD
            .get_or_init(|| Regex::new(r"[A-Za-z0-9_]+").unwrap())
            .find_iter(text)
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
                out.push_str(&text[cursor..words[i].start()]);
                out.push_str(name);
                cursor = words[i + count - 1].end();
                i += count;
            } else {
                i += 1;
            }
        }
        out.push_str(&text[cursor..]);
        out
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
        let notes = self.store.lock().unwrap().notes(meeting)?;
        Ok(Terms::new(&notes, &self.library_entries()?))
    }
    /// Archive the original ASR revision. Human edits and protected minutes remain authoritative.
    pub fn correct_terminology(&self, meeting: &str) -> Result<usize> {
        let terms = self.terminology(meeting)?;
        let mut db = self.store.lock().unwrap();
        let mut m = db.meeting(meeting)?;
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
                let text = terms.normalize(&original.text);
                if text != s.text {
                    db.edit_segment_text(&s.id, &text, s.revision, "terminology")?;
                    s.revision += 1;
                    changed += 1;
                }
                checked.push(s);
            }
            db.mark_terminology_batch(&checked, &terms_key)?;
        }
        db.set_cache_dependency(meeting, "terms", &terms_key)?;
        if changed > 0 {
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
        }
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
