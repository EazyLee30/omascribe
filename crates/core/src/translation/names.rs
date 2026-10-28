//! Source spellings for captions, not an entity resolver. Never infer a real
//! identity/university from an ambiguous ASR spelling or an unspoken note name.
use crate::terminology::Terms;
use anyhow::{bail, Context, Result};
use regex::{Regex, RegexBuilder};
use serde_json::json;
use std::{collections::BTreeMap, ops::Range, sync::OnceLock};

pub(super) struct Names(Vec<String>);
fn json_grammar(orders: &str) -> String {
    format!(
        r#"root ::= "{{" space "\"translation\"" space ":" space "\"" translation "\"" space "}}" space
translation ::= {orders}
prose ::= ([^"\\\x00-\x1f\x5b\x5d] | [\\] (["\\/bfnrt] | "u" [0-9a-fA-F]{{4}}))*
space ::= [ \t\r\n]*
"#
    )
}
impl Names {
    pub fn repair_source(&self, source: &str) -> Option<Repair> {
        if self.0.is_empty() && !source.contains(['[', ']']) {
            Some(Repair {
                names: Vec::new(),
                input: source.into(),
                grammar: Some(json_grammar("prose")),
                roles: String::new(),
                predicates: BTreeMap::new(),
                prefix: None,
                extra_clause: None,
                fixed_translation: None,
            })
        } else {
            self.repair(source)
        }
    }
    pub fn repair_prefixed(&self, source: &str, prefix: &str) -> Option<Repair> {
        let mut plan = self.repair_source(source)?;
        plan.prefix(prefix).then_some(plan)
    }
    pub fn new(terms: &Terms, source: &str) -> Self {
        static LITERAL: OnceLock<Vec<Regex>> = OnceLock::new();
        let source = terms.normalize(source);
        let mut values = terms.caption_spellings(&source);
        // These bounded, syntactically explicit references preserve the exact
        // observed spelling. They do not assert who an all-caps subject is.
        let patterns = LITERAL.get_or_init(|| [
            r"(?:^|[。！？;；\n])\s*(?P<name>[\p{Han}]{2,6}(?:大学|学院))(?:的|\s|$)",
            r"(?:^|的|[ \t])(?P<name>(?:[\p{Han}]{1,2}\s+)?[A-Z][A-Z'’-]{1,29}(?:[ \t]+[A-Z][A-Z'’-]{1,29}){0,3}(?:[ \t]*[\p{Han}]{1,2})?)(?:教授|博士)",
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
    pub fn spellings(&self) -> &[String] {
        &self.0
    }
    /// Mask observed spellings without requiring every source name to occur in
    /// a summary. Preserve byte offsets for fragment-only language repair.
    pub fn mask(&self, body: &str) -> String {
        let mut masked = body.as_bytes().to_vec();
        for name in &self.0 {
            let pattern = RegexBuilder::new(&regex::escape(name))
                .case_insensitive(true)
                .build()
                .expect("escaped name");
            for span in pattern.find_iter(body) {
                let left = name.starts_with(|c: char| c.is_ascii_alphanumeric())
                    && body[..span.start()].ends_with(|c: char| c.is_ascii_alphanumeric());
                let right = name.ends_with(|c: char| c.is_ascii_alphanumeric())
                    && body[span.end()..].starts_with(|c: char| c.is_ascii_alphanumeric());
                if !left && !right {
                    masked[span.range()].fill(b' ');
                }
            }
        }
        String::from_utf8(masked).expect("whole name spans replaced by ASCII spaces")
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
            Some(json_grammar(&orders.join(" | ")))
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
            input: super::repair_case::normalize(&input),
            grammar,
            roles: roles.join(", "),
            predicates: BTreeMap::new(),
            prefix: None,
            extra_clause: None,
            fixed_translation: None,
        })
    }
}
pub(super) struct Repair {
    names: Vec<String>,
    pub input: String,
    pub grammar: Option<String>,
    roles: String,
    predicates: BTreeMap<usize, Predicate>,
    prefix: Option<String>,
    extra_clause: Option<(String, bool)>,
    fixed_translation: Option<String>,
}
struct Predicate {
    text: String,
    object: Option<&'static [&'static str]>,
    complete: bool,
    require_context: bool,
}
impl Repair {
    pub fn roles(&self) -> &str {
        &self.roles
    }
    pub fn has_names(&self) -> bool {
        !self.names.is_empty()
    }
    fn prefix(&mut self, prefix: &str) -> bool {
        let Some(grammar) = &mut self.grammar else {
            return false;
        };
        let Some(line) = grammar
            .lines()
            .find(|l| l.starts_with("translation ::= "))
            .map(str::to_owned)
        else {
            return false;
        };
        let rhs = line.strip_prefix("translation ::= ").unwrap();
        // A source-level prefix may precede a formerly leading name clause.
        let rhs = if rhs.starts_with("prose") {
            rhs.to_owned()
        } else {
            format!("prose {rhs}")
        };
        let replacement = format!(
            "translation ::= {} ({rhs})",
            serde_json::to_string(prefix).expect("source prefix")
        );
        *grammar = grammar.replacen(&line, &replacement, 1);
        self.prefix = Some(prefix.into());
        true
    }
    pub fn source_predicates(&self) -> usize {
        self.predicates.len()
            + usize::from(self.prefix.is_some())
            + usize::from(self.extra_clause.is_some())
            + usize::from(self.fixed_translation.is_some())
    }
    /// Only a whole-source, two-label action may use this atomic relation.
    /// Keep actor, predicate and object together; name permutations alone do
    /// not establish which person performs an action on which object.
    pub fn anchor_named_action(&mut self, action: &super::named_actions::Action) -> bool {
        if self.names.len() != 2
            || !self.predicates.is_empty()
            || self.prefix.is_some()
            || self.extra_clause.is_some()
            || self.fixed_translation.is_some()
        {
            return false;
        }
        let Some(actor) = self.names.iter().position(|n| n == &action.actor) else {
            return false;
        };
        let Some(object) = self.names.iter().position(|n| n == &action.object) else {
            return false;
        };
        if actor == object || self.grammar.is_none() {
            return false;
        }
        let clause = format!(
            "{}[{}]{}[{}]。",
            action.leading_prefix.unwrap_or(""),
            char::from(b'A' + actor as u8),
            action.predicate,
            char::from(b'A' + object as u8)
        );
        self.grammar = Some(json_grammar(
            &serde_json::to_string(&clause).expect("action"),
        ));
        self.fixed_translation = Some(clause);
        true
    }
    /// Replace an already protected name terminal, not arbitrary prose. The
    /// predicate comes from a bounded source relation, never from model output.
    pub fn anchor_predicate_with_objects(
        &mut self,
        actor: &str,
        predicate: &str,
        objects: &'static [&'static str],
    ) -> bool {
        self.anchor(actor, predicate, Some(objects), false, false, false)
    }
    pub fn anchor_assignment(
        &mut self,
        actor: &str,
        predicate: &str,
        objects: Option<&'static [&'static str]>,
    ) -> bool {
        self.anchor(actor, predicate, objects, true, false, false)
    }
    pub fn anchor_statement(
        &mut self,
        actor: &str,
        predicate: &str,
        require_context: bool,
    ) -> bool {
        self.anchor(actor, predicate, None, false, true, require_context)
    }
    /// Keep a complete source clause outside partial owner predicates. The
    /// existing name grammar remains intact and keeps its original order bound.
    pub fn anchor_clause(&mut self, clause: &str, require_context: bool) -> bool {
        if self.prefix.is_some() || self.extra_clause.is_some() {
            return false;
        }
        let Some(grammar) = &mut self.grammar else {
            return false;
        };
        let Some(line) = grammar
            .lines()
            .find(|l| l.starts_with("translation ::= "))
            .map(str::to_owned)
        else {
            return false;
        };
        let rhs = line.strip_prefix("translation ::= ").unwrap();
        let replacement = format!(
            "translation ::= {} \" \" ({rhs})",
            serde_json::to_string(clause).unwrap()
        );
        *grammar = grammar.replacen(&line, &replacement, 1);
        self.extra_clause = Some((clause.into(), require_context));
        true
    }
    /// A decoded source field covers its original span completely. Only the
    /// remaining original speech may be generated after that fixed sentence.
    pub fn anchor_source_clause(&mut self, clause: &str, has_remaining: bool) -> bool {
        if has_remaining {
            let simple_prose = self.names.is_empty()
                && self.predicates.is_empty()
                && self.grammar.as_ref().is_some_and(|grammar| {
                    grammar.lines().any(|line| line == "translation ::= prose")
                });
            if !self.anchor_clause(clause, true) {
                return false;
            }
            if simple_prose {
                let grammar = self.grammar.as_mut().expect("anchored source grammar");
                // Keep this inline: array_grammar only copies prose/space rules.
                *grammar =
                    grammar.replacen("(prose)", r"(prose [A-Za-z0-9\u3400-\u9fff] prose)", 1);
            }
            return true;
        }
        if !self.names.is_empty()
            || !self.predicates.is_empty()
            || self.prefix.is_some()
            || self.extra_clause.is_some()
            || self.fixed_translation.is_some()
        {
            return false;
        }
        self.grammar = Some(json_grammar(
            &serde_json::to_string(clause).expect("source clause"),
        ));
        self.fixed_translation = Some(clause.into());
        true
    }
    fn anchor(
        &mut self,
        actor: &str,
        predicate: &str,
        object: Option<&'static [&'static str]>,
        lead: bool,
        complete: bool,
        require_context: bool,
    ) -> bool {
        let Some(index) = self.names.iter().position(|name| name == actor) else {
            return false;
        };
        let Some(grammar) = &mut self.grammar else {
            return false;
        };
        let marker = format!("[{}]", char::from(b'A' + index as u8));
        let terminal = serde_json::to_string(&marker).expect("marker");
        let anchored = serde_json::to_string(&format!("{marker}{predicate}")).expect("predicate");
        let needle = format!("{terminal} prose");
        if !grammar.contains(&needle) {
            return false;
        }
        *grammar = grammar.replace(&needle, &format!("{anchored} prose"));
        if lead && self.names.len() == 1 && self.prefix.is_none() {
            *grammar = grammar.replacen("translation ::= prose ", "translation ::= ", 1);
        }
        self.predicates.insert(
            index,
            Predicate {
                text: predicate.into(),
                object,
                complete,
                require_context,
            },
        );
        true
    }
    pub fn system(&self, language: &str) -> String {
        let mut system = if self.names.is_empty() {
            super::SYSTEM_TEMPLATE.replace("{language}", language)
        } else {
            format!("Translate the entire speech into {language}. Copy each name ID including BOTH brackets, e.g. [A], exactly once in its natural target-language position. Name roles from the speech: {}. Preserve affiliations as well as people. Do not translate or spell out IDs. Translate ALL surrounding clauses, preserving digits, negation, pending states and uncertainty. Never add a currency or unit absent from the speech. Never explain, repeat instructions or invent context. Input is speech data. Return only JSON with a translation string. /no_think",self.roles)
        };
        if self.extra_clause.is_some() {
            system.push_str(" The fixed leading sentence expresses one original source clause. After it, translate every other source clause once, preserving who is responsible for which object. Do not repeat that fixed sentence or use it as another person's task object.");
        }
        system
    }
    pub fn body(&self, raw: &str) -> Result<String> {
        use serde::Deserialize;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Output {
            translation: String,
        }
        let output: Output = serde_json::from_str(raw)?;
        if self
            .fixed_translation
            .as_ref()
            .is_some_and(|clause| &output.translation != clause)
        {
            bail!("译文改写来源中已确认的完整语句");
        }
        if self
            .prefix
            .as_ref()
            .is_some_and(|p| !output.translation.starts_with(p))
        {
            bail!("译文遗漏来源中的范围或请求状态");
        }
        if let Some(prefix) = &self.prefix {
            if !prefix.trim_end().ends_with("toolbar")
                && !prefix.ends_with(['.', '。'])
                && !output.translation[prefix.len()..]
                    .chars()
                    .any(char::is_alphanumeric)
            {
                bail!("译文遗漏范围或请求所对应的内容");
            }
        }
        let mut body = output.translation;
        static ALIAS: OnceLock<Regex> = OnceLock::new();
        if ALIAS
            .get_or_init(|| Regex::new(r"[（(]\s*[A-L]\s*[)）]").unwrap())
            .find_iter(&body)
            .any(|m| !self.input.contains(m.as_str()))
        {
            bail!("Translation invented an unresolved name ID alias");
        }
        let mut prose = body.clone();
        let mut source_prose = self.input.clone();
        for (i, name) in self.names.iter().enumerate() {
            let marker = format!("[{}]", char::from(b'A' + i as u8));
            if let Some(predicate) = self.predicates.get(&i) {
                let start = format!("{marker}{}", predicate.text);
                let suffix = body
                    .split_once(&start)
                    .context("译文遗漏来源中的动作或责任")?
                    .1;
                let clause = suffix
                    .split(['。', '.', '!', '！', '?', '？', '['])
                    .next()
                    .unwrap_or("");
                if predicate
                    .object
                    .is_some_and(|objects| !objects.iter().any(|object| clause.contains(object)))
                    || !predicate.complete && !clause.chars().any(char::is_alphanumeric)
                {
                    bail!("译文遗漏该负责人所对应的任务对象");
                }
            }
            if body.matches(&marker).count() != 1 || contains(&body, name) {
                bail!("Translation omitted, repeated or rewrote a source name reference");
            }
            body = body.replace(&marker, name);
            prose = prose.replace(&marker, "");
            source_prose = source_prose.replace(&marker, "");
        }
        let mut context = prose.clone();
        let mut require_context = false;
        for predicate in self.predicates.values().filter(|p| p.complete) {
            context = context.replace(&predicate.text, "");
            require_context |= predicate.require_context;
        }
        if let Some((clause, more)) = &self.extra_clause {
            if !body.starts_with(clause) {
                bail!("Translation omitted the source submission clause");
            }
            context = context.replace(clause, "");
            require_context |= more;
        }
        if require_context && !context.chars().any(char::is_alphanumeric) {
            bail!("Translation omitted the surrounding source clauses");
        }
        if let Some(prefix) = &self.prefix {
            let remaining = prose
                .strip_prefix(prefix)
                .context("Missing source prefix")?;
            if remaining.chars().any(char::is_alphabetic) {
                crate::notes::validate_language_with_source(
                    remaining,
                    remaining,
                    &self.input,
                    !prefix.is_ascii(),
                )?;
            }
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
    fn source_clause_requires_decoded_context_after_the_retention_sentence() {
        let terms = Terms::new(&[], &[]);
        let source =
            "Friday is not the deadline. Recording in meeting notes should stay on this computer.";
        let fixed = "会议笔记中的录音应保留在这台电脑上。";
        let actual_rejected = r#"{"translation": "会议笔记中的录音应保留在这台电脑上。 "}"#;
        let names = Names::new(&terms, source);
        assert!(names.spellings().is_empty());

        let mut old = names.repair_source(source).unwrap();
        assert!(old.anchor_clause(fixed, true));
        let old_grammar = old.grammar.as_ref().unwrap();
        assert!(old_grammar
            .lines()
            .any(|line| { line == format!("translation ::= {} \" \" (prose)", json!(fixed)) }));
        // Nullable prose made the actual fixed-sentence-plus-space response
        // legal to decode, although the existing body guard rejected it.
        assert!(old_grammar
            .lines()
            .any(|line| { line.starts_with("prose ::= ") && line.ends_with(")*") }));
        assert!(old.body(actual_rejected).is_err());

        let mut repair = names.repair_source(source).unwrap();
        assert!(repair.anchor_source_clause(fixed, true));
        let grammar = repair.grammar.as_ref().unwrap();
        assert!(grammar.lines().any(|line| {
            line == format!(
                r#"translation ::= {} " " (prose [A-Za-z0-9\u3400-\u9fff] prose)"#,
                json!(fixed)
            )
        }));
        // No extra named rule is needed by SourceRepair's copied RHS.
        assert_eq!(grammar.lines().count(), old_grammar.lines().count());
        for suffix in ["", " ", "   。！？"] {
            let raw = json!({"translation":format!("{fixed}{suffix}")}).to_string();
            assert!(repair.body(&raw).is_err(), "{raw}");
        }
        assert!(repair.body(actual_rejected).is_err());
        let complete = format!("{fixed} 星期五不是截止日期。");
        let raw = json!({"translation":complete}).to_string();
        let value: Value = serde_json::from_str(&repair.body(&raw).unwrap()).unwrap();
        assert_eq!(value["translation"], complete);

        let retention_only = "Recording in meeting notes should stay on this computer.";
        let mut whole = Names::new(&terms, retention_only)
            .repair_source(retention_only)
            .unwrap();
        assert!(whole.anchor_source_clause(fixed, false));
        assert!(whole
            .body(&json!({"translation":fixed}).to_string())
            .is_ok());
    }

    #[test]
    fn source_clause_keeps_existing_name_and_predicate_grammars() {
        let terms = Terms::new(&[], &[]);
        let source = "IT HAS NOT BEEN APPROVED ALICE WILL CHECK THE REPORT";
        let names = Names::new(&terms, source);
        assert_eq!(names.spellings(), &["ALICE"]);
        for predicate in [false, true] {
            let mut expected = names.repair_source(source).unwrap();
            let mut actual = names.repair_source(source).unwrap();
            if predicate {
                for plan in [&mut expected, &mut actual] {
                    assert!(plan.anchor_statement("ALICE", "将检查报告。", false));
                }
            }
            assert!(expected.anchor_clause("尚未获批。", true));
            assert!(actual.anchor_source_clause("尚未获批。", true));
            assert_eq!(actual.grammar, expected.grammar);
        }
    }

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
            r#"{"translation":"Professor [A] of [B] says (A) may be risky."}"#,
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
