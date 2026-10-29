//! A local role veto for explicit reports about a conditional participant.
//! Ordinary role labels are source spans, never new names or inferred identities.
use crate::terminology::Terms;
use anyhow::{ensure, Result};
use regex::Regex;
use std::{collections::BTreeSet, ops::Range, sync::OnceLock};

const MAX_BYTES: usize = 65_536;
const MAX_SOURCE_BYTES: usize = 262_144;
const MAX_FRAMES: usize = 24;
const REPORT_VERBS: &[&str] = &[
    "表示", "指出", "说明", "提醒", "声称", "强调", "提到", "认为", "说",
];
const DECISION_VERBS: &[&str] = &["决定", "决议", "选择", "确定"];

#[derive(Debug)]
pub(crate) struct Mismatch;
impl Mismatch {
    pub(crate) fn hint(&self) -> &'static str {
        "保留原话的发言或决定主体。条件或可能事件中被谈及的人物，不因此成为实际发言或决定者；新增归属须有对应原话支持。"
    }
}
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.hint())
    }
}
impl std::error::Error for Mismatch {}

#[derive(PartialEq, Eq)]
enum Predicate {
    Statement,
    Assessment,
    Decision,
}
struct Event {
    actor_start: usize,
    verb: Range<usize>,
    end: usize,
    predicate: Predicate,
    planned: bool,
    nonassertive: bool,
    ambiguous: bool,
    payload: String,
}

fn hard_boundary(c: char) -> bool {
    matches!(c, '。' | '.' | '；' | ';' | '!' | '！' | '?' | '？' | '\n')
}
fn boundary_at(text: &str, at: usize, ch: char) -> bool {
    hard_boundary(ch)
        && !(ch == '.'
            && text[..at].ends_with(|c: char| c.is_ascii_alphanumeric())
            && text[at + 1..].starts_with(|c: char| c.is_ascii_alphanumeric()))
}
fn prefix(text: &str, at: usize, soft: bool) -> &str {
    let start = text[..at]
        .char_indices()
        .rev()
        .find(|(i, ch)| boundary_at(text, *i, *ch) || soft && matches!(*ch, '，' | ','))
        .map_or(0, |(i, ch)| i + ch.len_utf8());
    text[start..at].trim()
}
fn quotes(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut open = None;
    for (at, ch) in text.char_indices() {
        if let Some((start, close)) = open {
            if ch == close {
                ranges.push(start..at + ch.len_utf8());
                open = None;
            }
        } else {
            let close = match ch {
                '“' => '”',
                '‘' => '’',
                '「' => '」',
                '『' => '』',
                '"' | '`' => ch,
                _ => continue,
            };
            open = Some((at, close));
        }
    }
    if let Some((start, _)) = open {
        ranges.push(start..text.len());
    }
    ranges
}
fn quoted(ranges: &[Range<usize>], at: usize) -> bool {
    ranges.iter().any(|range| range.contains(&at))
}
fn clauses(text: &str) -> Vec<Range<usize>> {
    let quotes = quotes(text);
    let mut start = 0;
    let mut result = Vec::new();
    for (at, ch) in text.char_indices() {
        if boundary_at(text, at, ch) && !quoted(&quotes, at) {
            let end = at + ch.len_utf8();
            if !text[start..end].trim().is_empty() {
                result.push(start..end);
            }
            start = end;
        }
    }
    if !text[start..].trim().is_empty() {
        result.push(start..text.len());
    }
    result
}
fn predicates() -> &'static Regex {
    static PREDICATES: OnceLock<Regex> = OnceLock::new();
    PREDICATES.get_or_init(|| {
        Regex::new(&format!(
            "(?:{}|{})",
            REPORT_VERBS.join("|"),
            DECISION_VERBS.join("|")
        ))
        .unwrap()
    })
}
fn opaque_names(text: &str, terms: &Terms) -> String {
    let mut mask = crate::translation::source_name_mask(text, text, terms).into_bytes();
    for (original, masked) in text.bytes().zip(&mut mask) {
        if original != *masked {
            *masked = b'#';
        }
    }
    String::from_utf8(mask).expect("whole source name spans become opaque ASCII")
}
fn conditional_actors(source: &str, syntax: &str) -> Result<Vec<String>> {
    static FRAME: OnceLock<Regex> = OnceLock::new();
    let frame =
        FRAME.get_or_init(|| {
            Regex::new(&format!(
        concat!(
            r"(?:^|[。.!?！？；;\n])\s*(?P<outer>[^。.!?！？；;，,\n]{{1,120}}?)",
            r#"(?:{})\s*[：:，,]?\s*["'“‘「『]?\s*"#,
            r"(?P<actor>[^。.!?！？；;，,：:\n]{{1,40}}?)(?:如果|假如|若|一旦|可能|也许|或许)"
        ), REPORT_VERBS.join("|")
    )).unwrap()
        });
    let mut actors = Vec::new();
    for capture in frame.captures_iter(syntax) {
        ensure!(actors.len() < MAX_FRAMES, Mismatch);
        let range = capture
            .name("actor")
            .expect("literal actor capture")
            .range();
        let actor = source[range].trim();
        if actor.is_empty()
            || !actor.chars().any(char::is_alphanumeric)
            || REPORT_VERBS
                .iter()
                .chain(["如果", "假如"].iter())
                .any(|cue| actor.contains(cue))
        {
            continue;
        }
        actors.push(actor.to_owned());
    }
    Ok(actors)
}
fn payload(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && !"，,：:。.;；!！?？\"“”‘’「」『』`".contains(*c))
        .collect::<String>()
        .to_ascii_lowercase()
}
fn event_pattern(actor: &str) -> Result<Regex> {
    Ok(Regex::new(&format!(
        concat!(
            r"(?P<actor>{})[ \t]*",
            r"(?P<modifiers>[^。.!?！？；;，,\n#]{{0,16}}?)",
            r"(?P<verb>{}|{})"
        ),
        regex::escape(actor),
        REPORT_VERBS.join("|"),
        DECISION_VERBS.join("|")
    ))?)
}
fn events(text: &str, pattern: &Regex) -> Result<Vec<Event>> {
    static MODIFIERS: OnceLock<Regex> = OnceLock::new();
    let known_modifiers = MODIFIERS.get_or_init(|| Regex::new(
        r"^(?:(?:随后|接着|明确|进一步|已经|刚刚|根据情况|计划|准备|没有|并未|从未|如果|假如|可能|也许|或许|否认|也|则|曾|已|将|会|未|不|没|若)[ \t]*){0,8}$"
    ).unwrap());
    let mut found = Vec::new();
    for capture in pattern.captures_iter(text) {
        ensure!(found.len() < MAX_FRAMES, Mismatch);
        let actor = capture.name("actor").expect("actor capture");
        let before = prefix(text, actor.start(), true);
        // Unknown neighboring syntax is retained as an ambiguous candidate,
        // never silently discarded or equated with a different literal label.
        let ambiguous_subject = text[..actor.start()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric())
            || text[..actor.start()]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric())
                && ![
                    "", "随后", "接着", "另外", "同时", "然后", "此外", "如果", "假如", "若",
                    "一旦",
                ]
                .contains(&before)
                && !REPORT_VERBS.iter().any(|verb| before.ends_with(verb));
        let verb = capture.name("verb").expect("verb capture");
        let end = text[verb.end()..]
            .char_indices()
            .find(|(i, ch)| boundary_at(text, verb.end() + *i, *ch))
            .map_or(text.len(), |(i, _)| verb.end() + i);
        let content = text[verb.end()..end].trim();
        if content.is_empty() || content.chars().count() > 240 {
            continue;
        }
        let modifiers = &capture["modifiers"];
        let nonassertive = [
            "没有", "并未", "从未", "未", "不", "没", "否认", "如果", "假如", "若", "可能", "也许",
            "或许",
        ]
        .iter()
        .any(|cue| modifiers.contains(cue))
            || ["如果", "假如", "若", "一旦", "除非"]
                .iter()
                .any(|cue| before.ends_with(cue))
            || text[end..].starts_with(['?', '？']);
        found.push(Event {
            actor_start: actor.start(),
            verb: verb.range(),
            end,
            predicate: if DECISION_VERBS.contains(&verb.as_str()) {
                Predicate::Decision
            } else if verb.as_str() == "认为" {
                Predicate::Assessment
            } else {
                Predicate::Statement
            },
            planned: ["将", "会", "计划", "准备"]
                .iter()
                .any(|cue| modifiers.contains(cue)),
            nonassertive,
            ambiguous: ambiguous_subject || !known_modifiers.is_match(modifiers),
            payload: payload(content),
        });
    }
    Ok(found)
}

struct AuthorWitness {
    actor_start: usize,
    verb: Range<usize>,
    payload: Range<usize>,
}
fn source_body(text: &str) -> &str {
    let text = text.trim();
    text.strip_suffix('。')
        .or_else(|| {
            text.strip_suffix('.')
                .filter(|prefix| !prefix.ends_with('.'))
        })
        .unwrap_or(text)
}
fn author_witnesses(
    body: &str,
    actor: &str,
    pattern: &Regex,
    sources: &[&str],
    authors: &[Option<&str>],
    syntax: &str,
    quoted_body: &[Range<usize>],
) -> Result<Vec<AuthorWitness>> {
    let mut result = Vec::new();
    let mut candidates = 0;
    for capture in pattern.captures_iter(body) {
        candidates += 1;
        ensure!(candidates <= MAX_FRAMES, Mismatch);
        let subject = capture.name("actor").expect("actor capture");
        let verb = capture.name("verb").expect("verb capture");
        if !capture["modifiers"].trim().is_empty()
            || !prefix(body, subject.start(), false).is_empty()
            || quoted(quoted_body, subject.start())
            || syntax.get(verb.range()) != body.get(verb.range())
            || verb.as_str() == "认为"
            || DECISION_VERBS.contains(&verb.as_str())
        {
            continue;
        }
        let mut start = verb.end();
        start += body[start..].len() - body[start..].trim_start().len();
        if let Some(ch @ (',' | '，' | ':' | '：')) = body[start..].chars().next() {
            start += ch.len_utf8();
            start += body[start..].len() - body[start..].trim_start().len();
        }
        for (source, author) in sources.iter().zip(authors) {
            if *author != Some(actor) {
                continue;
            }
            let expected = source_body(source);
            if expected.is_empty() || !body[start..].starts_with(expected) {
                continue;
            }
            let end = start + expected.len();
            // The reported source must finish here. A comma or extra payload
            // cannot turn a whole-source report into a new authored assertion.
            let after = &body[end..];
            let closes_source = after.chars().next().is_some_and(|ch| {
                ch == '\r' || matches!(ch, '。' | '.' | '\n') && boundary_at(body, end, ch)
            });
            if !after.trim().is_empty() && !closes_source {
                continue;
            }
            ensure!(result.len() < MAX_FRAMES, Mismatch);
            result.push(AuthorWitness {
                actor_start: subject.start(),
                verb: verb.range(),
                payload: start..end,
            });
            break;
        }
    }
    Ok(result)
}

/// Only explicit report/conditional-role frames are guarded. A witness is
/// parsed within one cited source; words from separate sources never form one.
/// This veto does not establish general subject or whole-article entailment.
pub(super) fn validate(body: &str, sources: &[&str], terms: &Terms) -> Result<()> {
    validate_inner(body, sources, None, terms)
}

pub(super) fn validate_with_authors(
    body: &str,
    sources: &[&str],
    authors: &[Option<&str>],
    terms: &Terms,
) -> Result<()> {
    ensure!(
        sources.len() <= 256 && sources.len() == authors.len(),
        Mismatch
    );
    ensure!(
        authors
            .iter()
            .flatten()
            .all(|author| !author.trim().is_empty()),
        Mismatch
    );
    validate_inner(body, sources, Some(authors), terms)
}

fn validate_inner(
    body: &str,
    sources: &[&str],
    authors: Option<&[Option<&str>]>,
    terms: &Terms,
) -> Result<()> {
    ensure!(body.len() <= MAX_BYTES && sources.len() <= 256, Mismatch);
    ensure!(
        sources
            .iter()
            .try_fold(0usize, |sum, source| {
                if source.len() > MAX_BYTES {
                    None
                } else {
                    sum.checked_add(source.len())
                }
            })
            .is_some_and(|bytes| bytes <= MAX_SOURCE_BYTES),
        Mismatch
    );
    let syntax: Vec<_> = sources
        .iter()
        .map(|source| opaque_names(source, terms))
        .collect();
    let mut actors = BTreeSet::new();
    for (source, mask) in sources.iter().zip(&syntax) {
        actors.extend(conditional_actors(source, mask)?);
        ensure!(actors.len() <= MAX_FRAMES, Mismatch);
    }
    let body_quotes = quotes(body);
    let body_syntax = opaque_names(body, terms);
    let body_clauses = clauses(body);
    let source_clauses: Vec<_> = sources
        .iter()
        .flat_map(|source| {
            clauses(source)
                .into_iter()
                .map(move |range| source[range].trim())
        })
        .collect();
    for actor in actors {
        // One bounded compilation per actor, shared by the body and all sources.
        let pattern = event_pattern(&actor)?;
        let parsed = events(body, &pattern)?;
        let authored = if let Some(authors) = authors {
            author_witnesses(
                body,
                &actor,
                &pattern,
                sources,
                authors,
                &body_syntax,
                &body_quotes,
            )?
        } else {
            Vec::new()
        };
        let mut witnesses = Vec::new();
        for (source, mask) in sources.iter().zip(&syntax) {
            let source_quotes = quotes(source);
            for event in events(source, &pattern)? {
                let before = prefix(source, event.actor_start, false);
                if event.nonassertive
                    || event.ambiguous
                    || quoted(&source_quotes, event.actor_start)
                    || ![
                        "", "随后", "接着", "另外", "同时", "然后", "此外", "并且", "但", "不过",
                    ]
                    .contains(&before)
                    || mask.get(event.verb.clone()) != source.get(event.verb.clone())
                {
                    continue;
                }
                // A question after a comma cannot become an asserted witness.
                if source[event.verb.end..event.end].contains(['?', '？']) {
                    continue;
                }
                witnesses.push(event);
            }
        }
        let mut candidates = 0;
        for clause in &body_clauses {
            let text = &body[clause.clone()];
            for (offset, _) in text.match_indices(actor.as_str()) {
                let at = clause.start + offset;
                let after = at + actor.len();
                // A later mention of this same role owns its own predicate.
                let end = body[after..clause.end]
                    .find(actor.as_str())
                    .map_or(clause.end, |next| after + next);
                let Some(verb) = predicates().find(&body_syntax[after..end]) else {
                    continue;
                };
                candidates += 1;
                ensure!(candidates <= MAX_FRAMES, Mismatch);
                let range = after + verb.start()..after + verb.end();
                let authored_source = authored.iter().any(|witness| {
                    witness.actor_start == at && witness.verb == range
                        || witness.payload.contains(&at) && range.end <= witness.payload.end
                });
                let structured = parsed
                    .iter()
                    .find(|event| event.actor_start == at && event.verb == range)
                    .is_some_and(|event| {
                        !event.ambiguous
                            && !event.nonassertive
                            && !quoted(&body_quotes, event.actor_start)
                            && !["如果", "假如", "若", "一旦", "除非", "假设", "并非", "不是"]
                                .iter()
                                .any(|cue| prefix(body, event.actor_start, false).starts_with(cue))
                            && witnesses.iter().any(|witness| {
                                witness.predicate == event.predicate
                                    && witness.planned == event.planned
                                    && witness.payload == event.payload
                            })
                    });
                // Unknown surrounding subject syntax, conditions, quotation or
                // overflow is opaque. Only the entire unchanged source clause
                // can authorize it; never strip its prefix or combine sources.
                ensure!(
                    authored_source || structured || source_clauses.contains(&text.trim()),
                    Mismatch
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str =
        "当迪大学的 PAMELA FURGO森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性";
    const SECOND: &str =
        "邓迪大学的潘 MALE FERGUSON教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性";
    const CONDITIONAL: &str = "审计员指出承包商如果上传日志可能泄露信息。";
    const ACTUAL: &str = "承包商随后表示日志必须先脱敏。";

    fn check(body: &str, sources: &[&str]) -> Result<()> {
        validate(body, sources, &Terms::new(&[], &[]))
    }

    #[test]
    fn both_recorded_8da_articles_cannot_promote_the_conditional_participant() {
        for body in [
            "当迪大学的 PAMELA FURGO 森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性时，记者表示对此有疑问。随后，有人指出即使主舞台的音乐可能已经结束，嘉年华的某些部分仍可能继续演奏音乐直到深夜。邓迪大学的潘 MALE FERGUSON 教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。",
            "当迪大学的 PAMELA FURGO 森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性时，记者表示将根据情况决定是否公布。同时，有人指出即使主舞台的音乐可能已经结束，嘉年华的某些部分仍可能继续演奏音乐直到深夜。大学的潘 MALE FERGUSON 教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。",
        ] {
            let error = check(body, &[FIRST, SECOND]).expect_err("the source contains no journalist statement");
            assert!(error.is::<Mismatch>());
        }
    }

    #[test]
    fn ordinary_roles_have_the_same_veto_without_becoming_names() {
        for body in [
            "承包商表示日志存在风险。",
            "承包商决定是否上传日志。",
            "承包商将根据情况决定是否上传日志。",
        ] {
            assert!(check(body, &[CONDITIONAL]).is_err(), "{body}");
        }
        assert!(check(
            "审计员指出，如果承包商上传日志，可能泄露信息。",
            &[CONDITIONAL]
        )
        .is_ok());
        assert!(check(
            "承包商表示日志存在风险。",
            &["审计员说明承包商可能上传日志。"]
        )
        .is_err());
        assert!(check(
            "承包商认为日志存在风险。",
            &["审计员认为承包商如果上传日志可能泄露信息。"]
        )
        .is_err());
        for original in [
            "审计员指出，承包商如果上传日志可能泄露信息。",
            "审计员指出：承包商如果上传日志可能泄露信息。",
            "审计员指出 ： 承包商如果上传日志可能泄露信息。",
            "审计员指出：“承包商如果上传日志可能泄露信息。”",
        ] {
            assert!(
                check("承包商表示日志存在风险。", &[original]).is_err(),
                "{original}"
            );
        }
    }

    #[test]
    fn affirmative_adverbs_do_not_hide_a_new_role_attribution() {
        for body in ["后来承包商表示日志存在风险。", "承包商仍表示日志存在风险。"]
        {
            assert!(check(body, &[CONDITIONAL]).is_err(), "{body}");
        }
        for body in [
            "另一承包商表示日志必须先脱敏。",
            "分包承包商表示日志必须先脱敏。",
            "承包商甲表示日志必须先脱敏。",
            "如果批准，承包商表示日志必须先脱敏。",
            "并非承包商表示日志必须先脱敏。",
            "不是承包商表示日志必须先脱敏。",
            "承包商在仔细研究当天全部日志和现场结果后表示日志必须先脱敏。",
        ] {
            assert!(check(body, &[CONDITIONAL, body]).is_ok(), "{body}");
            assert!(check(body, &[CONDITIONAL, ACTUAL]).is_err(), "{body}");
        }
        let long = format!("承包商表示{}。", "日志仍需核对".repeat(50));
        assert!(check(&long, &[CONDITIONAL]).is_err());
        assert!(check(&long, &[CONDITIONAL, &long]).is_ok());
        assert!(check(
            "承包商在仔细研究当天全部日志和现场结果后表示日志必须先脱敏。",
            &[CONDITIONAL]
        )
        .is_err());
    }

    #[test]
    fn an_independent_corresponding_statement_in_any_cited_source_is_allowed() {
        let body = "承包商表示日志必须先脱敏。";
        assert!(check(body, &[CONDITIONAL, ACTUAL]).is_ok());
        assert!(check(body, &[&format!("{CONDITIONAL}{ACTUAL}")]).is_ok());
        // The same raw statement is irrelevant until it is actually cited.
        assert!(check(body, &[CONDITIONAL]).is_err());
        assert!(check(
            "承包商认为日志必须先脱敏。",
            &[CONDITIONAL, "承包商认为日志必须先脱敏。"]
        )
        .is_ok());
        assert!(check(body, &[CONDITIONAL, "承包商认为日志必须先脱敏。"]).is_err());
        assert!(check(
            "承包商将决定是否上传日志。",
            &[CONDITIONAL, "承包商将决定是否上传日志。"]
        )
        .is_ok());
    }

    #[test]
    fn another_actor_or_unrelated_object_cannot_lend_permission() {
        for witness in [
            "工程师表示日志必须先脱敏。",
            "承包商表示发票必须先复核。",
            "承包商表示发票必须先复核。日志必须先脱敏。",
            "承包商表示发票必须先复核，工程师说明日志必须先脱敏。",
        ] {
            assert!(
                check("承包商表示日志必须先脱敏。", &[CONDITIONAL, witness]).is_err(),
                "{witness}"
            );
        }
        assert!(check("承包商决定日志必须先脱敏。", &[CONDITIONAL, ACTUAL]).is_err());
        assert!(check(
            "承包商表示日志必须先脱敏。承包商决定是否上传日志。",
            &[CONDITIONAL, ACTUAL]
        )
        .is_err());
        assert!(check(
            "承包商表示Node.rb仍需复核。",
            &[CONDITIONAL, "承包商表示Node.js仍需复核。"]
        )
        .is_err());
        assert!(check(
            "承包商表示日志必须先脱敏。",
            &[CONDITIONAL, "如果版本1.4获批，承包商表示日志必须先脱敏。"]
        )
        .is_err());
    }

    #[test]
    fn conditional_negative_question_or_quoted_events_are_not_asserted_witnesses() {
        for witness in [
            "如果承包商表示日志必须先脱敏，再确认。",
            "承包商可能表示日志必须先脱敏。",
            "承包商没有表示日志必须先脱敏。",
            "并非承包商表示日志必须先脱敏。",
            "承包商表示日志必须先脱敏吗？",
            "“承包商表示日志必须先脱敏”只是例句。",
            "假设承包商表示日志必须先脱敏。",
        ] {
            assert!(
                check("承包商表示日志必须先脱敏。", &[CONDITIONAL, witness]).is_err(),
                "{witness}"
            );
        }
        assert!(check(
            "承包商表示日志必须先脱敏。",
            &[
                "审计员指出承包商如果上传日志可能泄露信息",
                "表示日志必须先脱敏。"
            ]
        )
        .is_err());
    }

    #[test]
    fn quoted_name_data_cannot_supply_an_assertion_predicate() {
        let terms = Terms::new(
            &[],
            &[crate::library::Entry {
                kind: "term".into(),
                title: "承包商表示日志必须先脱敏".into(),
                enabled: true,
                ..Default::default()
            }],
        );
        assert!(validate(
            "承包商说日志必须先脱敏。",
            &[CONDITIONAL, "产品名称是承包商表示日志必须先脱敏。"],
            &terms
        )
        .is_err());
        assert!(validate(
            "产品名称是“承包商表示日志必须先脱敏”。",
            &[CONDITIONAL],
            &terms
        )
        .is_ok());
    }

    #[test]
    fn aligned_author_only_reports_their_whole_unchanged_quote() {
        let terms = Terms::new(&[], &[]);
        let original = "教授指出记者如果公布照片可能存在危害性。";
        let body = format!("记者表示，{original}");
        assert!(validate(&body, &[original], &terms).is_err());
        assert!(validate_with_authors(&body, &[original], &[Some("记者")], &terms).is_ok());
        assert!(validate_with_authors(
            &format!("记者说：{}", source_body(original)),
            &[original],
            &[Some("记者")],
            &terms
        )
        .is_ok());
        for changed in [
            "记者表示将根据情况决定是否公布。".to_string(),
            "记者表示，教授指出记者公布照片存在危害性。".to_string(),
            format!("记者决定，{original}"),
            format!("记者认为，{original}"),
            format!("记者将表示，{original}"),
            format!("记者表示，{}，因此决定公布。", source_body(original)),
            format!("记者表示，{original}记者决定公布照片。"),
        ] {
            assert!(
                validate_with_authors(&changed, &[original], &[Some("记者")], &terms).is_err(),
                "{changed}"
            );
        }
        assert!(validate_with_authors(&body, &[original], &[], &terms).is_err());
        assert!(validate_with_authors(&body, &[original], &[None], &terms).is_err());
        assert!(validate_with_authors(&body, &[original], &[Some("教授")], &terms).is_err());
        assert!(validate_with_authors(
            &body,
            &["音乐可能结束。", original],
            &[Some("记者"), None],
            &terms
        )
        .is_err());
        let quoted = "教授指出：“记者如果公布照片，可能存在危害性。”";
        assert!(validate_with_authors(
            &format!("记者表示，{quoted}"),
            &[quoted],
            &[Some("记者")],
            &terms
        )
        .is_ok());
        assert!(validate_with_authors(
            "记者表示，教授指出：记者如果公布照片，可能存在危害性。",
            &[quoted],
            &[Some("记者")],
            &terms
        )
        .is_err());
        for (source, suffix) in [
            ("教授指出记者如果公布照片可能存在危害性，工具为Node", ".js"),
            ("教授指出记者如果公布照片可能存在危害性，版本为1", ".4"),
        ] {
            assert!(
                validate_with_authors(
                    &format!("记者表示，{source}{suffix}"),
                    &[source],
                    &[Some("记者")],
                    &terms
                )
                .is_err(),
                "a source suffix must not be treated as a sentence boundary: {suffix}"
            );
        }
    }
}
