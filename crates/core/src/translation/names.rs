//! Source spellings for captions, not an entity resolver. Never infer a real
//! identity/university from an ambiguous ASR spelling or an unspoken note name.
use crate::terminology::Terms;
use anyhow::{bail, ensure, Context, Result};
use regex::{Regex, RegexBuilder};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::OnceLock,
};

// Match the existing original-evidence byte ceiling, including final merged
// captions. This is a local scan bound, not a model/context/token allowance.
const REPORTED_SCAN_BYTES: usize = 120_000;
const REPORTED_COUNT: usize = 12;
const REPORTED_CHARS: usize = 240;

/// Lexical name labels only. A title, quote or conditional occurrence does not
/// establish that this person spoke, decided, attended or owns any task.
struct ReportedSpans {
    ranges: Vec<Range<usize>>,
    truncated: bool,
}
fn reported_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            // Reuse the explicit mixed-script professor/doctor spelling frame.
            // Horizontal whitespace is literal; never bind across source lines.
            Regex::new(r"(?:^|的|[ \t。！？;；\n.!?])(?P<name>(?:[\p{Han}]{1,2}[ \t]+)?[A-Z][A-Za-z'’-]{1,79}(?:[ \t]+[A-Z][A-Za-z'’-]{1,79})*?(?:[ \t]*[\p{Han}][\p{Han}A-Za-z'’-]{0,79}?)??)(?:教授|博士)").unwrap(),
            // Target-language titles/verbs can change while the literal label
            // remains intact. A report/affiliation delimiter bounds the label.
            Regex::new(r"(?:^|[^A-Za-z0-9_])(?i:professor|prof\.|doctor|dr\.)[ \t]+(?P<name>(?:[\p{Han}]{1,2}[ \t]+)?[A-Za-z][A-Za-z'’-]{1,79}(?:[ \t]+[A-Za-z][A-Za-z'’-]{1,79})*?(?:[ \t]*[\p{Han}][\p{Han}A-Za-z'’-]{0,79}?)??)[ \t]+(?:(?i:said|says|stated|states|reported|reports|noted|notes|indicated|indicates|explained|explains|confirmed|confirms|pointed[ \t]+out|points[ \t]+out)\b|of\b|from\b)").unwrap(),
        ]
    })
}
fn reported_ranges(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    reported_patterns()
        .iter()
        .enumerate()
        .flat_map(move |(pattern, re)| {
            re.captures_iter(text).filter_map(move |capture| {
                let name = capture.name("name")?;
                // A regex must not start halfway through a longer capitalized
                // label and thereby authorize its suffix as a different person.
                if pattern == 0 {
                    let prefix = text[..name.start()].trim_end_matches([' ', '\t']);
                    let previous = prefix
                        .rsplit(|c: char| !c.is_alphanumeric() && !"'’-".contains(c))
                        .next()
                        .unwrap_or("");
                    if previous.starts_with(|c: char| c.is_ascii_uppercase()) {
                        return None;
                    }
                } else if name
                    .as_str()
                    .split_whitespace()
                    .next()
                    .is_some_and(|first| {
                        [
                            "said",
                            "says",
                            "stated",
                            "states",
                            "reported",
                            "reports",
                            "noted",
                            "notes",
                            "confirmed",
                            "confirms",
                            "indicated",
                            "indicates",
                            "explained",
                            "explains",
                            "of",
                            "from",
                            "the",
                            "a",
                            "an",
                        ]
                        .iter()
                        .any(|word| first.eq_ignore_ascii_case(word))
                    })
                {
                    return None;
                }
                Some(name.range())
            })
        })
}
fn reported_spans(text: &str) -> ReportedSpans {
    // A large plain caption needs no reported-person protection. Avoid both a
    // regex name scan and a new rejection for unrelated long speech.
    static TITLE: OnceLock<Regex> = OnceLock::new();
    if !TITLE
        .get_or_init(|| {
            Regex::new(r"教授|博士|(?i:\bprofessor\b|\bprof\.|\bdoctor\b|\bdr\.)").unwrap()
        })
        .is_match(text)
    {
        return ReportedSpans {
            ranges: Vec::new(),
            truncated: false,
        };
    }
    if text.len() > REPORTED_SCAN_BYTES {
        return ReportedSpans {
            ranges: Vec::new(),
            truncated: true,
        };
    }
    let mut ranges = Vec::new();
    let mut seen = BTreeSet::new();
    let mut remaining = REPORTED_CHARS;
    let mut truncated = false;
    for range in reported_ranges(text) {
        let name = &text[range.clone()];
        if !seen.insert(name) {
            continue;
        }
        let length = name.chars().count();
        if ranges.len() == REPORTED_COUNT || length > remaining {
            truncated = true;
            continue;
        }
        remaining -= length;
        ranges.push(range);
    }
    ReportedSpans { ranges, truncated }
}
pub(super) fn reported_data(text: &str) -> serde_json::Value {
    let data = reported_spans(text);
    json!({"spellings":data.ranges.into_iter().map(|range| &text[range]).collect::<Vec<_>>(),
        "truncated":data.truncated})
}
pub(super) fn validate_reported(
    source: &str,
    body: &str,
    canonical: impl Fn(&str) -> String,
) -> Result<()> {
    let candidates = reported_spans(body);
    ensure!(
        !candidates.truncated,
        "纪要报道名称校验超出有界范围，原文已保留"
    );
    if candidates.ranges.is_empty() {
        return Ok(());
    }
    ensure!(
        source.len() <= REPORTED_SCAN_BYTES,
        "纪要来源名称校验超出有界范围，原文已保留"
    );
    for range in candidates.ranges {
        let candidate = &body[range];
        // Scan the complete selected source, not the truncated prompt legend.
        // This is spelling membership, never identity or claim authorization.
        ensure!(
            reported_ranges(source).any(|range| {
                let observed = &source[range];
                candidate == observed || candidate == canonical(observed)
            }),
            "纪要改写了原话中的报道人名：{candidate}。保留来源中的专有名称拼写，不猜测其他姓名。"
        );
    }
    Ok(())
}

pub(super) struct Names(Vec<String>, bool);
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
        if self.1 {
            return None;
        }
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
        let reported = reported_spans(&source);
        values.extend(
            reported
                .ranges
                .iter()
                .map(|range| source[range.clone()].to_owned()),
        );
        // These bounded, syntactically explicit references preserve the exact
        // observed spelling. They do not assert who an all-caps subject is.
        let patterns = LITERAL.get_or_init(|| [
            r"(?:^|[。！？;；\n])\s*(?P<name>[\p{Han}]{2,6}(?:大学|学院))(?:的|\s|$)",
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
        let incomplete = reported.truncated
            || reported.ranges.iter().any(|range| {
                !selected
                    .iter()
                    .any(|name| contains(name, &source[range.clone()]))
            });
        Self(selected, incomplete)
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
        ensure!(!self.1, "字幕报道名称校验超出有界范围，原文已保留");
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
        if self.1
            || self.0.is_empty()
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
    /// A decoded source field covers its entire original span. No free tail
    /// may replace unidentified surrounding speech or invent another policy.
    pub fn anchor_source_clause(&mut self, clause: &str) -> bool {
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
    fn focused_terms(content: &str) -> Terms {
        Terms::new(
            &[crate::UserNote {
                id: "focus".into(),
                meeting_id: "meeting".into(),
                kind: "document".into(),
                content: content.into(),
                anchor_ms: 0,
                updated_at: String::new(),
            }],
            &[],
        )
    }

    #[test]
    fn reported_labels_are_exact_utf8_spans_and_not_generic_roles() {
        for (source, spelling) in [
            ("北岭大学的 LENA FARO森教授指出可能有风险。", "LENA FARO森"),
            (
                "研究所的沈 MALE DUNSTON博士表示仍未确认。",
                "沈 MALE DUNSTON",
            ),
            (
                "Professor LENA FARO森 stated that the report is pending.",
                "LENA FARO森",
            ),
            (
                "Doctor 沈 MALE DUNSTON reported that the report is pending.",
                "沈 MALE DUNSTON",
            ),
        ] {
            let data = reported_spans(source);
            assert!(!data.truncated);
            assert_eq!(data.ranges.len(), 1, "{source}");
            assert_eq!(&source[data.ranges[0].clone()], spelling);
        }
        for ordinary in [
            "THE SYSTEM REPORTS THE RESULT",
            "The Budget is pending.",
            "工程师表示缓存需要复核。",
            "教授指出仍需核实。",
            "博士表示还没确认。",
            "The professor reports that the test is pending.",
            "Professor reports that the test is pending.",
        ] {
            let data = reported_data(ordinary);
            assert_eq!(data["spellings"], json!([]), "{ordinary}");
            assert_eq!(data["truncated"], false);
        }
    }

    #[test]
    fn reported_spelling_membership_is_one_way_and_checks_whole_labels() {
        let source =
            "北岭大学的 LENA FARO森教授指出仍未确认。研究所的沈 MALE DUNSTON博士表示尚未完成。";
        let exact = |name: &str| name.to_owned();
        for body in [
            "Professor LENA FARO森 stated that the report remains unconfirmed.",
            "Doctor 沈 MALE DUNSTON noted that the test is pending.",
            "事项仍未确认。", // A summary need not retain every historical name.
        ] {
            validate_reported(source, body, exact).unwrap();
        }
        for body in [
            "LENA FARO 森教授指出仍未确认。",
            "SEN MALE DUNSTON博士表示仍未确认。",
            "Professor LENA FARO stated that the report is pending.",
            "Professor LENA FARO森X stated that the report is pending.",
            "Professor ZORA NIL stated that the report is pending.",
        ] {
            assert!(validate_reported(source, body, exact).is_err(), "{body}");
        }
        let other = "ALICE SMITH教授指出需要复核。";
        assert!(validate_reported(other, "ALICE教授指出需要复核。", exact).is_err());
        assert!(validate_reported("ALICE教授指出需要复核。", other, exact).is_err());
    }

    #[test]
    fn prompt_overflow_does_not_turn_complete_source_membership_into_a_partial_legend() {
        let source = (0..14)
            .map(|i| format!("NAME{}教授指出仍待确认。", char::from(b'A' + i)))
            .collect::<String>();
        let data = reported_data(&source);
        assert_eq!(data["spellings"].as_array().unwrap().len(), REPORTED_COUNT);
        assert_eq!(data["truncated"], true);
        // The final label is outside the prompt legend but still present in
        // the complete selected evidence. Do not reject a legitimate summary.
        validate_reported(&source, "NAMEN教授指出仍待确认。", str::to_owned).unwrap();
        validate_reported(&source, "仍待确认。", str::to_owned).unwrap();
        assert!(validate_reported(&source, "UNKNOWN教授指出仍待确认。", str::to_owned).is_err());
        assert!(validate_reported(&source, &source, str::to_owned).is_err());
        let names = Names::new(&Terms::new(&[], &[]), &source);
        assert!(
            names.validate(&source).is_err(),
            "caption overflow is not complete protection"
        );
        assert!(names.repair_source(&source).is_none());
        let oversized = format!("NAMEN教授指出仍待确认。{}", "x".repeat(REPORTED_SCAN_BYTES));
        assert_eq!(reported_data(&oversized)["truncated"], true);
        assert!(validate_reported(&oversized, "NAMEN教授指出仍待确认。", str::to_owned).is_err());
    }

    #[test]
    fn captions_keep_reported_labels_without_freezing_titles_or_report_verbs() {
        let source = "LENA FARO森教授指出报告仍未确认。";
        let names = Names::new(&Terms::new(&[], &[]), source);
        assert_eq!(names.spellings(), &["LENA FARO森"]);
        assert!(names
            .validate("Professor LENA FARO森 stated that the report is still unconfirmed.")
            .is_ok());
        assert!(names
            .validate("Professor LENA FARO 森 stated that the report is still unconfirmed.")
            .is_err());
    }

    #[test]
    fn final_caption_selection_must_not_drop_short_reported_names() {
        for identifiers in [
            (0..12)
                .map(|i| format!("ModelIdentifierAA{i}"))
                .collect::<Vec<_>>(),
            (0..6)
                .map(|i| format!("LongID{}{}{i}", "x".repeat(32), char::from(b'A' + i)))
                .collect(),
        ] {
            let source = format!("LI博士表示仍待确认。 {}", identifiers.join(" "));
            let reported = reported_spans(&source);
            assert!(!reported.truncated);
            assert_eq!(reported.ranges.len(), 1);
            let names = Names::new(&Terms::new(&[], &[]), &source);
            assert!(!names.spellings().iter().any(|name| name == "LI"));
            assert!(names.validate(&source).is_err());
            assert!(names.repair_source(&source).is_none());
        }
    }

    #[test]
    fn long_plain_captions_do_not_require_reported_name_protection() {
        let source = "The report is still pending. ".repeat(REPORTED_SCAN_BYTES / 20 + 1);
        assert!(source.len() > REPORTED_SCAN_BYTES);
        assert_eq!(
            reported_data(&source),
            json!({"spellings":[],"truncated":false})
        );
        let names = Names::new(&Terms::new(&[], &[]), &source);
        assert!(names.validate("报告仍未确认。").is_ok());
        assert!(names.repair_source(&source).is_some());
    }
    #[test]
    fn actual_withdrawal_caption_preserves_focused_teacher_spelling_without_gender_guess() {
        let terms = focused_terms("**展览** **预算** **刘老师**");
        let source =
            "周五开放的安排撤回了，新日期还没决定，刘老师继续负责布置，预算改为5000元但仍未确定。";
        let names = Names::new(&terms, source);
        // Actual C8 phase 2 caption: test the spelling error independently of
        // its action wording, which must not be guessed by a name repair.
        let observed = "The schedule for Friday is canceled, the new date has not been decided, Mr. Liu continues to be responsible for assigning, and the budget has been changed to 5000 yuan but has not been confirmed.";
        assert_eq!(names.spellings(), &["刘老师"]);
        assert!(names.validate(observed).is_err());
        assert!(names
            .validate(&observed.replace("Mr. Liu", "王老师"))
            .is_err());
        assert!(names
            .validate(&observed.replace("Mr. Liu", "刘老师"))
            .is_ok());
        let repair = names.repair(source).unwrap();
        assert!(repair.input.contains("[A]继续负责布置"));
        assert!(!repair.input.contains("刘老师"));
        let raw = json!({"translation": observed.replace("Mr. Liu", "[A]")}).to_string();
        let restored: Value = serde_json::from_str(&repair.body(&raw).unwrap()).unwrap();
        assert_eq!(
            restored["translation"],
            observed.replace("Mr. Liu", "刘老师")
        );
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
