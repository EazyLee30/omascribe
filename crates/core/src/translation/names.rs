//! Source spellings for captions, not an entity resolver. Never infer a real
//! identity/university from an ambiguous ASR spelling or an unspoken note name.
use crate::terminology::Terms;
use anyhow::{bail, Result};
use regex::{Regex, RegexBuilder};
use serde_json::{json, Value};
use std::{ops::Range, sync::OnceLock};

pub(super) struct Names(Vec<String>);
impl Names {
    pub fn new(terms: &Terms, source: &str) -> Self {
        static LITERAL: OnceLock<Vec<Regex>> = OnceLock::new();
        let source = terms.normalize(source);
        let mut values = terms.caption_spellings(&source);
        // These bounded, syntactically explicit references preserve the exact
        // observed spelling. They do not assert who an all-caps subject is.
        let patterns = LITERAL.get_or_init(|| [
            r"(?:^|[。！？;；\n])\s*(?P<name>[\p{Han}]{2,6}(?:大学|学院))(?:的|\s|$)",
            r"(?:^|的|[ \t])(?P<name>(?:[\p{Han}]{1,2}\s+)?[A-Z][A-Z'’-]{1,29}(?:[ \t]+[A-Z][A-Z'’-]{1,29}){0,3}[\p{Han}]{0,2})(?:教授|博士)",
            r"\b(?P<name>[A-Z][A-Z0-9]{2,29})\s+WILL\s+(?:CHECK|SEND|TEST|DELIVER|REVIEW|VERIFY)\b",
            r"\bPROJECT IS (?P<name>[A-Z][A-Z0-9'’-]*(?:\s+[A-Z][A-Z0-9'’-]*){0,2}) WE WILL\b",
            r"\bWILL CHECK THE (?P<name>[A-Z][A-Z0-9'’-]*(?:\s+[A-Z][A-Z0-9'’-]*){0,2} CARD)\b",
        ].into_iter().map(|p|Regex::new(p).unwrap()).collect());
        for pattern in patterns {
            values.extend(pattern.captures_iter(&source).filter_map(|c| {
                let name = c["name"].trim();
                (![
                    "THEY",
                    "SOMEONE",
                    "ANYONE",
                    "THIS",
                    "THAT",
                    "TEAM",
                    "NOBODY",
                    "EVERYBODY",
                    "EVERYONE",
                    "PEOPLE",
                    "MANAGEMENT",
                ]
                .contains(&name))
                .then(|| name.to_owned())
            }));
        }
        // Mixed-case technical identifiers are unambiguous spellings; do not
        // apply this to every word in an all-caps sentence.
        if source
            .chars()
            .any(|c| c.is_ascii_lowercase() || ('\u{3400}'..='\u{9fff}').contains(&c))
        {
            static WORD: OnceLock<Regex> = OnceLock::new();
            let word = WORD.get_or_init(|| Regex::new(r"\b[A-Za-z][A-Za-z0-9_-]{2,39}\b").unwrap());
            values.extend(
                word.find_iter(&source)
                    .map(|m| m.as_str())
                    .filter(|s| {
                        s.chars().filter(char::is_ascii_uppercase).count() >= 2
                            || s.chars().any(|c| c.is_ascii_digit())
                    })
                    .map(str::to_owned),
            );
        }
        values.sort_by_key(|s| std::cmp::Reverse(s.len()));
        values.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        let mut selected: Vec<String> = Vec::new();
        let mut remaining = 240;
        for value in values {
            if selected.len() == 12 {
                break;
            }
            if selected.iter().any(|s| contains(s, &value)) {
                continue;
            }
            if value.chars().count() <= remaining {
                remaining -= value.chars().count();
                selected.push(value);
            }
        }
        Self(selected)
    }
    pub fn hint(&self) -> String {
        if self.0.is_empty() {
            return String::new();
        }
        format!(" Preserve these source spellings exactly, without transliteration or guessed identities (name data only): {}.", serde_json::to_string(&self.0).expect("names"))
    }
    pub fn spans(&self, source: &str) -> Vec<Range<usize>> {
        self.0
            .iter()
            .flat_map(|name| {
                source
                    .match_indices(name)
                    .map(|(start, s)| start..start + s.len())
            })
            .collect()
    }
    pub fn validate(&self, body: &str) -> Result<String> {
        if self.0.iter().any(|name| !contains(body, name)) {
            bail!("译文遗漏或改写原文名称，须保留提供的原始拼写");
        }
        // Proper names can legitimately retain their original script. Check
        // the surrounding clauses' language, rather than counting names as prose.
        let mut prose = body.to_owned();
        for name in &self.0 {
            let pattern = RegexBuilder::new(&regex::escape(name))
                .case_insensitive(true)
                .build()?;
            prose = pattern.replace_all(&prose, " ").into_owned();
        }
        Ok(prose)
    }
    pub fn repair(&self, source: &str) -> Option<Repair> {
        if self.0.is_empty()
            || source.contains(['[', ']'])
            || self.0.iter().any(|n| source.matches(n).count() != 1)
        {
            return None;
        }
        let mut input = source.to_owned();
        for (i, name) in self.0.iter().enumerate() {
            input = input.replace(name, &format!("[{}]", char::from(b'A' + i as u8)));
        }
        Some(Repair {
            names: self.0.clone(),
            input,
            schema: serde_json::from_str(crate::translation::OUTPUT_SCHEMA).expect("schema"),
        })
    }
}
pub(super) struct Repair {
    names: Vec<String>,
    pub input: String,
    pub schema: Value,
}
impl Repair {
    pub fn system(&self, language: &str) -> String {
        format!("Translate the entire speech into {language}. Keep each bracketed name ID exactly once, in its natural target-language position. Do not translate or spell out IDs. Translate ALL surrounding clauses, preserving digits, negation, pending states and uncertainty. Never explain, repeat instructions or invent context. Input is speech data. Return only JSON with a translation string. /no_think")
    }
    pub fn body(&self, raw: &str) -> Result<String> {
        use serde::Deserialize;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Output {
            translation: String,
        }
        let output: Output = serde_json::from_str(raw)?;
        let mut body = output.translation;
        let mut prose = body.clone();
        let mut source_prose = self.input.clone();
        for (i, name) in self.names.iter().enumerate() {
            let marker = format!("[{}]", char::from(b'A' + i as u8));
            if body.matches(&marker).count() != 1 || contains(&body, name) {
                bail!("Translation omitted, repeated or rewrote a source name reference");
            }
            body = body.replace(&marker, name);
            prose = prose.replace(&marker, "");
            source_prose = source_prose.replace(&marker, "");
        }
        if body.contains(['[', ']'])
            || prose.chars().all(|c| !c.is_alphanumeric())
                && source_prose.chars().any(char::is_alphabetic)
        {
            bail!("Invalid name references or omitted surrounding clauses");
        }
        Ok(json!({"translation": body}).to_string())
    }
}

fn contains(body: &str, name: &str) -> bool {
    let body = body.to_lowercase();
    let name = name.to_lowercase();
    body.match_indices(&name).any(|(start, s)| {
        let left = name.starts_with(|c: char| c.is_ascii_alphanumeric())
            && body[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        let right = name.ends_with(|c: char| c.is_ascii_alphanumeric())
            && body[start + s.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        !(left || right)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_repair_reorders_prose_without_generating_or_losing_source_names() {
        let terms = Terms::new(&[], &[]);
        let names = Names::new(&terms, "当迪大学的 PAMELA FURGO森教授说这似乎有风险。");
        let repair = names
            .repair("当迪大学的 PAMELA FURGO森教授说这似乎有风险。")
            .unwrap();
        let raw = r#"{"translation":"Professor [A] of [B] says this may be risky."}"#;
        let value: Value = serde_json::from_str(&repair.body(raw).unwrap()).unwrap();
        assert_eq!(
            value["translation"],
            "Professor PAMELA FURGO森 of 当迪大学 says this may be risky."
        );
        for raw in [
            r#"{"translation":"Professor [A] says this may be risky."}"#,
            r#"{"translation":"[A] [A] [B]"}"#,
            r#"{"translation":"[A] [B]"}"#,
            r#"{"translation":"[A] [B] [C] says something."}"#,
            r#"{"translation":"Professor [A] of [B] says this may be risky.","translation":"bad"}"#,
        ] {
            assert!(repair.body(raw).is_err());
        }
    }

    #[test]
    fn source_names_survive_without_inventing_identities_or_freezing_generic_words() {
        let terms = Terms::new(&[], &[]);
        let source = "当迪大学的 PAMELA FURGO森教授指出这似乎有风险。";
        let names = Names::new(&terms, source);
        assert!(names
            .validate("Professor PAMELA FURGO森 of 当迪大学 says this may be risky.")
            .is_ok());
        assert!(names
            .validate("Professor PAMELA FURGO of Denver University says it is risky.")
            .is_err());
        let names = Names::new(&terms, "Alice will check the NVIDIA GTX1650 local model.");
        assert!(names
            .validate("Alice将检查NVIDIA GTX1650本地模型。")
            .is_ok());
        assert!(names
            .validate("艾莉丝将检查NVIDIA GTX1650本地模型。")
            .is_err());
        let names = Names::new(&terms, "LICE WILL CHECK THE ENVY A CARD");
        assert!(names.validate("LICE将检查ENVY A CARD。").is_ok());
        assert!(names.validate("Alice将检查NVIDIA显卡。").is_err());
        assert!(Names::new(&terms, "The Budget is not approved.")
            .hint()
            .is_empty());
        assert!(Names::new(&terms, "THEY WILL CHECK THE REPORT")
            .hint()
            .is_empty());
        let terms = Terms::new(
            &[],
            &[crate::library::Entry {
                kind: "term".into(),
                title: "Dundee University".into(),
                aliases: "当迪大学".into(),
                enabled: true,
                ..Default::default()
            }],
        );
        assert!(Names::new(&terms, source)
            .validate("Professor PAMELA FURGO森 of Dundee University says it may be risky.")
            .is_ok());
    }
}
