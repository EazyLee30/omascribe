//! Revisioned, extractive fact/state records. No omitted update deletes an older fact.
//! Model assignments are hypotheses; exact spans and lifecycle checks are deterministic,
//! while the factual corpus is still required for subject/meaning correctness.
#[cfg(test)]
mod probe;
mod protocol;
#[cfg(test)]
mod tests;
use crate::{Segment, Snapshot};
use anyhow::{ensure, Context, Result};
pub(crate) use protocol::{compose_schema, extraction_schema, extraction_system};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

pub(crate) const KEY: &str = "minutes-facts-v1";
pub(crate) const MAX_FACTS: usize = 64;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Amount,
    Owner,
    Deadline,
    Status,
    Statement,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Reported,
    Uncertain,
    Confirmed,
    Planned,
    Incomplete,
    Completed,
    Withdrawn,
    Question,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Evidence {
    pub segment: String,
    pub revision: u32,
    pub start: usize,
    pub end: usize,
}
impl Evidence {
    pub fn quote<'a>(&self, snapshot: &'a Snapshot) -> Result<&'a str> {
        let source = snapshot
            .segments
            .iter()
            .find(|s| s.id == self.segment && s.revision == self.revision)
            .context("Fact evidence revision is no longer current")?;
        source
            .text
            .get(self.start..self.end)
            .filter(|s| !s.trim().is_empty())
            .context("Invalid fact evidence span")
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fact {
    pub id: u32,
    pub version: u32,
    pub subject: String,
    pub kind: Kind,
    pub value: String,
    pub state: State,
    pub evidence: Vec<Evidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_names: Vec<String>,
}
#[derive(Debug, Clone)]
pub(crate) struct Clause {
    pub evidence: Evidence,
    pub text: String,
    pub speaker: Option<String>,
}
pub(crate) fn clauses(snapshot: &Snapshot, segments: &[Segment]) -> Vec<Clause> {
    let mut result = vec![];
    for s in segments {
        let chars: Vec<_> = s.text.char_indices().collect();
        let mut start = 0;
        let mut push = |a: usize, b: usize| {
            let raw = &s.text[a..b];
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return;
            }
            let left = raw.len() - raw.trim_start().len();
            result.push(Clause {
                evidence: Evidence {
                    segment: s.id.clone(),
                    revision: s.revision,
                    start: a + left,
                    end: a + left + trimmed.len(),
                },
                text: trimmed.into(),
                speaker: s
                    .speaker
                    .as_ref()
                    .and_then(|id| snapshot.meeting.speaker_names.get(id))
                    .cloned(),
            });
        };
        for (i, &(offset, c)) in chars.iter().enumerate() {
            let numeric = ['.', ','].contains(&c)
                && i > 0
                && chars
                    .get(i + 1)
                    .is_some_and(|(_, next)| next.is_ascii_digit())
                && chars[i - 1].1.is_ascii_digit();
            if !numeric
                && ['。', '；', '，', '！', '？', ';', ',', '.', '!', '?', '\n'].contains(&c)
            {
                let end = offset + c.len_utf8();
                push(start, end);
                start = end;
            }
        }
        push(start, s.text.len());
    }
    result
}
fn numeric_values(text: &str) -> HashSet<String> {
    static NUMBERS: OnceLock<regex::Regex> = OnceLock::new();
    NUMBERS
        .get_or_init(|| regex::Regex::new(r"\d+(?:[.,]\d+)*").unwrap())
        .find_iter(text)
        .map(|m| m.as_str().replace(',', ""))
        .collect()
}
fn lower_contains(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}
fn clause_text(text: &str) -> &str {
    text.trim()
        .trim_end_matches(['。', '；', '，', '！', '？', ';', ',', '.', '!', '?'])
        .trim()
}
fn cue(state: State, text: &str) -> bool {
    let text = text.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
    match state {
        State::Reported => true,
        State::Uncertain => has(&[
            "未定",
            "未确定",
            "未决定",
            "没决定",
            "没有确定",
            "还没",
            "待定",
            "待确认",
            "暂定",
            "暂按",
            "尚未批准",
            "尚未获批",
            "not approved",
            "unapproved",
            "unconfirmed",
            "not decided",
            "undecided",
            "pending",
            "tentative",
        ]),
        State::Confirmed => {
            has(&[
                "最终",
                "已确定",
                "确定为",
                "已确认",
                "决定为",
                "approved",
                "confirmed",
                "agreed",
                "final amount",
            ]) && !has(&[
                "未",
                "没",
                "不",
                "not ",
                "unapproved",
                "unconfirmed",
                "?",
                "？",
            ])
        }
        State::Planned => has(&[
            "将",
            "计划",
            "准备",
            "预计",
            "打算",
            "待",
            "will ",
            "plan",
            "scheduled",
            "intend",
            "propos",
        ]),
        State::Incomplete => has(&[
            "未完成",
            "没完成",
            "尚未进行",
            "尚未实现",
            "还没有完成",
            "not completed",
            "not finished",
            "not yet",
            "still pending",
        ]),
        State::Completed => {
            static COMPLETED: OnceLock<regex::Regex> = OnceLock::new();
            (has(&["已完成", "已经完成", "完成了"])
                || COMPLETED
                    .get_or_init(|| regex::Regex::new(r"\b(completed|finished|done)\b").unwrap())
                    .is_match(&text))
                && !has(&[
                    "未", "没", "不", "not ", "no ", "nobody", "never", "will ", "计划", "将", "?",
                    "？",
                ])
        }
        State::Withdrawn => has(&["撤回", "取消", "不再", "withdraw", "cancel", "no longer"]),
        State::Question => has(&["?", "？", "是否", "为什么", "why ", "whether", "how "]),
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ledger {
    pub facts: Vec<Fact>,
    pub next_id: u32,
    pub cursor: u32,
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            facts: vec![],
            next_id: 1,
            cursor: 0,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Updates {
    updates: Vec<Update>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    id: String,
    subject: String,
    kind: Kind,
    value: String,
    state: State,
    sources: Vec<usize>,
}
impl Ledger {
    pub fn visible_ids(&self, input: &[Clause]) -> Vec<u32> {
        let text = input
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        self.facts
            .iter()
            .filter(|f| lower_contains(&text, &f.subject))
            .map(|f| f.id)
            .collect()
    }
    pub fn prompt(&self, input: &[Clause]) -> Value {
        json!({"current":self.facts.iter().filter(|f|self.visible_ids(input).contains(&f.id)).map(|f|json!({"id":format!("f{}",f.id),"subject":f.subject,"kind":f.kind,"value":f.value,"state":f.state})).collect::<Vec<_>>(),
            "new":input.iter().enumerate().map(|(i,c)|json!({"id":i+1,"text":c.text,"speaker":c.speaker})).collect::<Vec<_>>()})
    }
    /// Construct the entire candidate in memory. A parse/coverage/field error changes nothing.
    pub fn apply(
        &self,
        snapshot: &Snapshot,
        input: &[Clause],
        encoded: &str,
        cursor: u32,
    ) -> Result<Self> {
        ensure!(
            encoded.len() <= 32_768,
            "Fact update response exceeds budget"
        );
        ensure!(cursor >= self.cursor, "Fact frontier cannot move backwards");
        ensure!(
            snapshot.segments.last().is_some_and(|s| cursor <= s.seq),
            "Fact frontier exceeds current source coverage"
        );
        let pending: Vec<_> = snapshot
            .segments
            .iter()
            .filter(|s| s.seq > self.cursor && s.seq <= cursor)
            .cloned()
            .collect();
        let expected: HashSet<_> = clauses(snapshot, &pending)
            .into_iter()
            .map(|c| c.evidence)
            .collect();
        let received: HashSet<_> = input.iter().map(|c| c.evidence.clone()).collect();
        ensure!(
            expected == received,
            "Fact batch omitted a source prefix or clause"
        );
        ensure!(
            self.facts
                .iter()
                .flat_map(|f| &f.evidence)
                .all(|e| e.quote(snapshot).is_ok()),
            "An older fact source was revised; rebuild is required"
        );
        let proposed: Updates =
            serde_json::from_str(encoded).context("Invalid fact update schema")?;
        ensure!(proposed.updates.len() <= 32, "Too many fact updates");
        for clause in input {
            ensure!(
                clause.evidence.quote(snapshot)? == clause.text,
                "Fact source span changed"
            );
        }
        let mut next = self.clone();
        let mut seen = HashSet::new();
        let mut covered = HashSet::new();
        let admits = protocol::admission(self, input);
        for patch in proposed.updates {
            ensure!(
                !patch.subject.trim().is_empty()
                    && patch.subject.chars().count() <= 64
                    && !patch.value.trim().is_empty()
                    && patch.value.chars().count() <= 320,
                "Invalid fact field size"
            );
            ensure!(
                !patch.sources.is_empty() && patch.sources.len() <= 8,
                "Fact update needs bounded original evidence"
            );
            let cited: Vec<_> = patch
                .sources
                .iter()
                .map(|id| {
                    input
                        .get(id.checked_sub(1).context("Invalid fact source ID")?)
                        .context("Unknown fact source ID")
                })
                .collect::<Result<_>>()?;
            for clause in &cited {
                ensure!(
                    clause.evidence.quote(snapshot)? == clause.text,
                    "Fact source span changed"
                );
            }
            if let Some(id) = patch
                .id
                .strip_prefix('f')
                .and_then(|id| id.parse::<u32>().ok())
            {
                if self.facts.iter().any(|old| {
                    old.id == id
                        && old.subject == patch.subject
                        && old.kind == patch.kind
                        && old.value == patch.value
                        && old.state == patch.state
                }) {
                    continue;
                }
            }
            ensure!(
                admits(&patch),
                "Fact source, literal value, state and field target are not a supported combination"
            );
            let target_id = if patch.id == "new" {
                0
            } else {
                patch
                    .id
                    .strip_prefix('f')
                    .context("Invalid fact target prefix")?
                    .parse::<u32>()?
            };
            let target = if target_id == 0 {
                None
            } else {
                Some(
                    self.facts
                        .iter()
                        .find(|f| f.id == target_id)
                        .context("Unknown fact target")?,
                )
            };
            if let Some(old) = target {
                ensure!(
                    old.subject == patch.subject && old.kind == patch.kind,
                    "Fact updates cannot switch subject or field"
                );
                ensure!(
                    cited.iter().any(|c| snapshot
                        .segments
                        .iter()
                        .find(|s| s.id == c.evidence.segment)
                        .is_some_and(|s| lower_contains(&s.text, &old.subject))),
                    "Ambiguous fact target; preserve the previous state"
                );
            } else {
                ensure!(
                    cited
                        .iter()
                        .any(|c| lower_contains(&c.text, &patch.subject)),
                    "New fact subject is absent from original evidence"
                );
                if patch.kind != Kind::Statement {
                    ensure!(
                        !next.facts.iter().any(|f| f.kind == patch.kind
                            && f.subject.eq_ignore_ascii_case(&patch.subject)),
                        "Existing scalar field must be updated by ID"
                    );
                }
            }
            let first_person_owner = patch.kind == Kind::Owner
                && cited.iter().any(|c| {
                    c.speaker.as_deref() == Some(patch.value.as_str())
                        && ["我负责", "我来", "i will", "i own", "i am responsible"]
                            .iter()
                            .any(|v| lower_contains(&c.text, v))
                });
            ensure!(
                first_person_owner || cited.iter().any(|c| c.text.contains(&patch.value)),
                "Fact value is not an extractive source value"
            );
            if [Kind::Statement, Kind::Status].contains(&patch.kind) {
                ensure!(
                    cited
                        .iter()
                        .any(|c| clause_text(&c.text) == clause_text(&patch.value)),
                    "Statement/status must retain a complete source clause"
                );
            }
            ensure!(
                patch.state == State::Reported || cited.iter().any(|c| cue(patch.state, &c.text)),
                "Fact state has no matching original-language evidence"
            );
            if patch.kind == Kind::Status && patch.state != State::Reported {
                ensure!(
                    cue(patch.state, &patch.value),
                    "Status value itself contradicts the proposed state"
                );
            }
            if target_id != 0 && seen.contains(&target_id) {
                let current = next
                    .facts
                    .iter()
                    .find(|f| f.id == target_id)
                    .context("Missing current fact")?;
                if current.subject == patch.subject
                    && current.kind == patch.kind
                    && current.value == patch.value
                    && current.state == patch.state
                {
                    // Repeated identical proposals cannot bump a version, claim
                    // extra evidence, or consume uncaptured source clauses.
                    continue;
                }
                if patch.state == State::Withdrawn
                    && matches!(patch.kind, Kind::Owner | Kind::Amount | Kind::Deadline)
                    && target.is_some_and(|old| old.value == patch.value)
                    && current.value != patch.value
                {
                    // A withdrawal names the old slot value. It cannot revoke a
                    // different value assigned earlier in this candidate. Keep
                    // the literal historical withdrawal as its own sourced claim.
                    let clause = cited
                        .iter()
                        .find(|c| c.text.contains(&patch.value) && cue(State::Withdrawn, &c.text))
                        .context("Withdrawal does not name its historical value")?;
                    let id = next.next_id;
                    next.next_id = next.next_id.checked_add(1).context("Fact ID exhausted")?;
                    next.facts.push(Fact {
                        id,
                        version: 1,
                        subject: patch.subject,
                        kind: Kind::Statement,
                        value: clause.text.clone(),
                        state: State::Withdrawn,
                        evidence: vec![clause.evidence.clone()],
                        required_names: if patch.kind == Kind::Owner {
                            vec![patch.value.clone()]
                        } else {
                            vec![]
                        },
                    });
                    // Only the retained literal clause is covered; supporting
                    // context with other information must survive independently.
                    for (index, input) in input.iter().enumerate() {
                        if input.evidence == clause.evidence {
                            covered.insert(index + 1);
                        }
                    }
                    continue;
                }
                anyhow::bail!("Conflicting updates target the same current fact");
            }
            if target_id != 0 {
                seen.insert(target_id);
            }
            let mut evidence: Vec<_> = cited.iter().map(|c| c.evidence.clone()).collect();
            let explicit_state = if patch.state == State::Reported
                && patch.kind == Kind::Status
                && cue(State::Completed, &patch.value)
            {
                State::Completed
            } else if patch.state == State::Reported
                && cited
                    .iter()
                    .any(|c| c.text.contains(&patch.value) && cue(State::Confirmed, &c.text))
            {
                State::Confirmed
            } else {
                patch.state
            };
            let state = if let Some(old) = target.filter(|old| {
                explicit_state == State::Reported
                    && [State::Uncertain, State::Incomplete].contains(&old.state)
            }) {
                // A new amount does not by itself approve a budget or finish its review.
                evidence.extend(old.evidence.clone());
                old.state
            } else {
                explicit_state
            };
            let mut unique = HashSet::new();
            evidence.retain(|e| unique.insert(e.clone()));
            let id = if let Some(old) = target {
                old.id
            } else {
                let id = next.next_id;
                next.next_id = next.next_id.checked_add(1).context("Fact ID exhausted")?;
                id
            };
            let required_names = if patch.kind == Kind::Owner {
                vec![patch.value.clone()]
            } else {
                vec![]
            };
            let fact = Fact {
                id,
                version: target.map_or(1, |old| old.version.saturating_add(1)),
                subject: patch.subject,
                kind: patch.kind,
                value: patch.value,
                state,
                evidence,
                required_names,
            };
            for &id in &patch.sources {
                covered.insert(id);
            }
            if let Some(index) = next.facts.iter().position(|f| f.id == id) {
                next.facts[index] = fact;
            } else {
                next.facts.push(fact);
            }
        }
        // Never silently discard a source clause the small extractor did not understand.
        for (index, clause) in input.iter().enumerate() {
            if covered.contains(&(index + 1)) || super::writer::non_substantive(&clause.text) {
                continue;
            }
            let id = next.next_id;
            next.next_id = next.next_id.checked_add(1).context("Fact ID exhausted")?;
            next.facts.push(Fact {
                id,
                version: 1,
                subject: clause.text.chars().take(64).collect(),
                kind: Kind::Statement,
                value: clause.text.clone(),
                state: State::Reported,
                evidence: vec![clause.evidence.clone()],
                required_names: vec![],
            });
        }
        ensure!(
            next.facts.len() <= MAX_FACTS && serde_json::to_vec(&next)?.len() <= 64 * 1024,
            "Fact ledger exceeds bounded active-state budget"
        );
        next.cursor = cursor;
        Ok(next)
    }
    fn current_claim<'a>(fact: &Fact, snapshot: &'a Snapshot) -> Result<&'a str> {
        // Keep the actual current clause: a bare owner/value can otherwise lose
        // the action or turn a person's withdrawal into a cancelled proposal.
        let quotes = fact
            .evidence
            .iter()
            .map(|e| e.quote(snapshot))
            .collect::<Result<Vec<_>>>()?;
        quotes
            .iter()
            .copied()
            .find(|quote| quote.contains(&fact.value))
            .or_else(|| quotes.first().copied())
            .context("Fact has no current source claim")
    }
    pub fn composition_input(&self, snapshot: &Snapshot) -> Result<Value> {
        let facts=self.facts.iter().map(|f|Ok(json!({"id":f.id,"subject":f.subject,"kind":f.kind,"value":f.value,
            "state":f.state,"claim":Self::current_claim(f,snapshot)?,"required_names":f.required_names}))).collect::<Result<Vec<_>>>()?;
        Ok(json!({"facts":facts}))
    }
    /// Produce semantic validation atoms from current fields, then restore actual original
    /// full-segment citations after validation. Superseded amounts are not current facts.
    pub fn document(
        &self,
        snapshot: &Snapshot,
        encoded: &str,
        language: &str,
    ) -> Result<Vec<crate::Suggestion>> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Draft {
            sections: Vec<Section>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Section {
            body: String,
            facts: Vec<u32>,
        }
        let draft: Draft = serde_json::from_str(encoded)?;
        ensure!(
            !draft.sections.is_empty() && draft.sections.len() <= 2,
            "Fact article needs one or two connected paragraphs"
        );
        let lookup: HashMap<_, _> = self.facts.iter().map(|f| (f.id, f)).collect();
        let mut covered = HashSet::new();
        let mut paragraphs = vec![];
        let mut atoms = vec![];
        static CURRENCY: OnceLock<regex::Regex> = OnceLock::new();
        let currency = CURRENCY.get_or_init(|| regex::Regex::new(r"[$€£¥]\s*\d").unwrap());
        for section in draft.sections {
            ensure!(!section.facts.is_empty(), "Unbound article paragraph");
            for marker in currency.find_iter(&section.body) {
                let symbol = marker.as_str().chars().next().unwrap();
                let supported = section
                    .facts
                    .iter()
                    .filter_map(|id| lookup.get(id))
                    .flat_map(|f| &f.evidence)
                    .any(|e| e.quote(snapshot).is_ok_and(|quote| quote.contains(symbol)));
                ensure!(
                    supported,
                    "Article added a currency prefix absent from its literal evidence"
                );
            }

            let mut sources = vec![];
            for id in section.facts {
                let fact = lookup.get(&id).context("Article cites an unknown fact")?;
                covered.insert(id);
                ensure!(
                    fact.evidence.iter().all(|e| e.quote(snapshot).is_ok()),
                    "Fact evidence was revised before composition"
                );
                let source = snapshot
                    .segments
                    .iter()
                    .find(|s| s.id == fact.evidence[0].segment)
                    .context("Missing fact source")?;
                let mut atom = source.clone();
                atom.id = format!("fact:{id}");
                let state = match fact.state {
                    State::Reported => "",
                    State::Uncertain => "not approved 未确定",
                    State::Confirmed => "confirmed 已确定",
                    State::Planned => "will 计划",
                    State::Incomplete => "not completed 尚未完成",
                    State::Completed => "completed 已完成",
                    State::Withdrawn => "withdrawn 已撤回",
                    State::Question => "question 是否",
                };
                let state = if matches!(fact.kind, Kind::Statement | Kind::Status) {
                    ""
                } else {
                    state
                };
                atom.text = format!(
                    "{} {} {} {state}",
                    fact.subject,
                    fact.value,
                    Self::current_claim(fact, snapshot)?
                );
                for name in &fact.required_names {
                    ensure!(
                        lower_contains(&section.body, name),
                        "Article omitted a sourced person/team: {name}"
                    );
                }
                atoms.push(atom);
                sources.push(atoms.len());
                if fact.kind == Kind::Owner {
                    ensure!(
                        lower_contains(&section.body, &fact.value),
                        "Article omitted a current owner"
                    );
                }
                if fact.kind == Kind::Amount {
                    let expected = numeric_values(&fact.value);
                    let present = numeric_values(&section.body);
                    ensure!(
                        !expected.is_empty() && expected.is_subset(&present)
                            || expected.is_empty() && section.body.contains(&fact.value),
                        "Article omitted a current amount"
                    );
                }
            }
            paragraphs.push(json!({"title":if super::writer::chinese(snapshot,language){"会议纪要"}else{"Meeting minutes"},"body":section.body,"sources":sources}));
        }
        let missing: Vec<_> = self
            .facts
            .iter()
            .filter(|f| !covered.contains(&f.id))
            .map(|f| f.id)
            .collect();
        ensure!(
            missing.is_empty(),
            "Article omitted active fact IDs: {missing:?}"
        );
        let content = json!({"sections":paragraphs}).to_string();
        let body = paragraphs
            .iter()
            .filter_map(|v| v["body"].as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        super::writer::validate_language(&body, super::writer::chinese(snapshot, language))?;
        let mut sections = super::validation::validate_with_speakers(
            &content,
            &atoms,
            &snapshot.meeting.id,
            &snapshot.meeting.speaker_names,
        )?;
        for section in &mut sections {
            let mut originals = vec![];
            for atom in &section.sources {
                let id = atom
                    .segment_id
                    .strip_prefix("fact:")
                    .context("Invalid fact binding")?
                    .parse::<u32>()?;
                for evidence in &lookup.get(&id).context("Missing fact binding")?.evidence {
                    let s = snapshot
                        .segments
                        .iter()
                        .find(|s| s.id == evidence.segment && s.revision == evidence.revision)
                        .context("Fact original changed")?;
                    if originals
                        .iter()
                        .any(|q: &crate::Source| q.segment_id == s.id)
                    {
                        continue;
                    }
                    originals.push(crate::Source {
                        segment_id: s.id.clone(),
                        revision: s.revision,
                        start_ms: s.start_ms,
                        end_ms: s.end_ms,
                        quote: s.text.clone(),
                        speaker_name: s
                            .speaker
                            .as_ref()
                            .and_then(|id| snapshot.meeting.speaker_names.get(id))
                            .cloned(),
                    });
                }
            }
            section.sources = originals;
        }
        Ok(sections)
    }
}
