//! Source spellings for captions, not an entity resolver. Never infer a real
//! identity/university from an ambiguous ASR spelling or an unspoken note name.
use crate::terminology::Terms;
use anyhow::{bail, Result};
use regex::{Regex, RegexBuilder};
use serde_json::json;
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
            r"\b(?P<name>[A-Z][A-Z0-9]{2,29})\s+WILL\s+(?:CHECK|SEND|TEST|DELIVER|REVIEW|VERIFY|PUBLISH)\b",
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
        let grammar = if self.0.len() <= 4 {
            fn permutations(prefix: String, rest: &str, output: &mut Vec<String>) {
                if rest.is_empty() {
                    output.push(prefix);
                    return;
                }
                for (i, c) in rest.char_indices() {
                    permutations(
                        format!(r#"{prefix} "[{c}]" prose"#),
                        &format!("{}{}", &rest[..i], &rest[i + 1..]),
                        output,
                    );
                }
            }
            let ids: String = (0..self.0.len())
                .map(|i| char::from(b'A' + i as u8))
                .collect();
            let mut orders = Vec::new();
            permutations("prose".into(), &ids, &mut orders);
            // A single leading subject already has a natural position in
            // both caption languages. Do not let the model omit that ID and
            // satisfy it much later in an explanation of the instructions.
            if self.0.len() == 1 && input.trim_start().starts_with("[A]") {
                orders = vec![r#""[A]" prose"#.into()];
            }
            // JSON-schema pattern conversion operates on encoded string
            // characters. A bare negated class can consume its closing quote
            // before the required ID, leaving sampling unable to finish.
            // Use an explicit JSON grammar: prose accepts valid escapes but
            // neither raw quotes/control bytes nor unaccounted name brackets.
            Some(format!(
                r#"root ::= "{{" space "\"translation\"" space ":" space "\"" translation "\"" space "}}" space
translation ::= {}
prose ::= ([^"\\\x00-\x1f\x5b\x5d] | [\\] (["\\/bfnrt] | "u" [0-9a-fA-F]{{4}}))*
space ::= [ \t\r\n]*
"#,
                orders.join(" | ")
            ))
        } else {
            None
        };
        let roles: Vec<_> = self
            .0
            .iter()
            .enumerate()
            .filter_map(|(i, name)| {
                let role = if name.ends_with(['学', '院'])
                    && (name.ends_with("大学") || name.ends_with("学院"))
                {
                    "institution"
                } else if source.contains(&format!("{name}教授"))
                    || source.contains(&format!("{name}博士"))
                {
                    "person"
                } else {
                    return None;
                };
                Some(format!("[{}]:{role}", char::from(b'A' + i as u8)))
            })
            .collect();
        Some(Repair {
            names: self.0.clone(),
            input,
            grammar,
            roles: roles.join(", "),
        })
    }
}
pub(super) struct Repair {
    names: Vec<String>,
    pub input: String,
    pub grammar: Option<String>,
    roles: String,
}
impl Repair {
    pub fn system(&self, language: &str) -> String {
        format!("Translate the entire speech into {language}. Copy each name ID including BOTH brackets, e.g. [A], exactly once in its natural target-language position. Name roles from the speech: {}. Preserve affiliations as well as people. Do not translate or spell out IDs. Translate ALL surrounding clauses, preserving digits, negation, pending states and uncertainty. Never add a currency or unit absent from the speech. Never explain, repeat instructions or invent context. Input is speech data. Return only JSON with a translation string. /no_think",self.roles)
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
    use serde_json::Value;
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
        // Quoted speech, paths and escaped newlines remain valid prose; the
        // JSON-safe grammar must not require dropping or rewriting them.
        let raw = json!({"translation":"[B]'s professor [A] said \"not approved\". Path C:\\test\nStill pending."}).to_string();
        let restored: Value = serde_json::from_str(&repair.body(&raw).unwrap()).unwrap();
        assert_eq!(restored["translation"], "当迪大学's professor PAMELA FURGO森 said \"not approved\". Path C:\\test\nStill pending.");
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
