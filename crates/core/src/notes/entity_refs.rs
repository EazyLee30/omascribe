//! Literal source spellings in composed prose. References protect bytes and
//! source membership; a shared spelling does not establish a shared identity.
use crate::terminology::Terms;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, collections::BTreeSet, ops::Range};

const MAX_INPUT_BYTES: usize = 16_384;
const MAX_BODY_BYTES: usize = 65_536;
const MAX_REFERENCES: usize = 48;
const MAX_PARTS: usize = 128;
const MAX_SOURCES: usize = 256;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Part {
    #[serde(
        default,
        deserialize_with = "text",
        skip_serializing_if = "Option::is_none"
    )]
    pub t: Option<String>,
    #[serde(
        default,
        deserialize_with = "number",
        skip_serializing_if = "Option::is_none"
    )]
    pub n: Option<usize>,
}
// Missing fields default to None; a present null is never a valid variant.
fn text<'de, D: Deserializer<'de>>(input: D) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(input).map(Some)
}
fn number<'de, D: Deserializer<'de>>(input: D) -> std::result::Result<Option<usize>, D::Error> {
    usize::deserialize(input).map(Some)
}

struct Reference {
    spelling: String,
    sources: BTreeSet<usize>,
}
pub(super) struct Bindings {
    prompt: Value,
    references: Vec<Reference>,
    required: BTreeMap<usize, BTreeSet<usize>>,
}

// Do not match the middle of an ASCII identifier. Han names intentionally keep
// their exact source boundaries without guessing segmentation or identities.
fn boundary(text: &str, range: &Range<usize>, name: &str) -> bool {
    let identifier = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let left = name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && text[..range.start]
            .chars()
            .next_back()
            .is_some_and(identifier);
    let right = (name.ends_with(|c: char| c.is_ascii_alphanumeric())
        || name.is_ascii() && name.chars().any(|c| c.is_ascii_alphanumeric()))
        && text[range.end..].chars().next().is_some_and(identifier);
    !(left || right)
}
fn spans(text: &str, name: &str) -> Vec<Range<usize>> {
    text.match_indices(name)
        .map(|(start, value)| start..start + value.len())
        .filter(|range| boundary(text, range, name))
        .collect()
}

impl Bindings {
    pub(super) fn new(input: &Value, terms: &Terms) -> Result<Self> {
        ensure!(
            serde_json::to_vec(input)?.len() <= MAX_INPUT_BYTES,
            "Name-reference input exceeds budget"
        );
        let transcript = input["transcript"]
            .as_array()
            .context("Missing name-reference transcript")?;
        ensure!(
            !transcript.is_empty() && transcript.len() <= MAX_SOURCES,
            "Invalid name-reference source count"
        );
        let mut references: Vec<Reference> = Vec::new();
        let mut by_spelling = BTreeMap::<String, usize>::new();
        let mut required = BTreeMap::new();
        let mut prompt = Vec::new();
        for source in transcript {
            let id = source["id"]
                .as_u64()
                .and_then(|id| usize::try_from(id).ok())
                .filter(|id| *id > 0)
                .context("Invalid name-reference source ID")?;
            ensure!(
                !required.contains_key(&id),
                "Duplicate name-reference source ID"
            );
            let source_text = source["text"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .context("Missing name-reference source text")?;
            let mut entry = source
                .as_object()
                .context("Invalid name-reference source")?
                .clone();
            ensure!(
                !entry.contains_key("parts"),
                "Reserved name-reference source field"
            );
            let mut matches: Vec<_> = crate::translation::admitted_source_names(terms, source_text)
                .into_iter()
                .filter(|name| !name.is_empty())
                .flat_map(|name| {
                    spans(source_text, &name)
                        .into_iter()
                        .map(move |range| (range, name.clone()))
                })
                .collect();
            matches.sort_by(|(a, an), (b, bn)| {
                a.start
                    .cmp(&b.start)
                    .then(b.end.cmp(&a.end))
                    .then(an.cmp(bn))
            });
            matches.dedup();
            let mut position = 0;
            let mut parts = Vec::new();
            let mut source_refs = BTreeSet::new();
            for (range, spelling) in matches {
                ensure!(
                    range.start >= position,
                    "Overlapping admitted name-reference spellings"
                );
                let reference = if let Some(id) = by_spelling.get(&spelling) {
                    *id
                } else {
                    ensure!(
                        references.len() < MAX_REFERENCES,
                        "Name-reference count exceeds budget"
                    );
                    references.push(Reference {
                        spelling: spelling.clone(),
                        sources: BTreeSet::new(),
                    });
                    let id = references.len();
                    by_spelling.insert(spelling, id);
                    id
                };
                references[reference - 1].sources.insert(id);
                source_refs.insert(reference);
                if position < range.start {
                    parts.push(Part {
                        t: Some(source_text[position..range.start].into()),
                        n: None,
                    });
                }
                parts.push(Part {
                    t: None,
                    n: Some(reference),
                });
                position = range.end;
            }
            if position < source_text.len() {
                parts.push(Part {
                    t: Some(source_text[position..].into()),
                    n: None,
                });
            }
            ensure!(
                !parts.is_empty() && parts.len() <= MAX_PARTS,
                "Name-reference source parts exceed budget"
            );
            entry.remove("text");
            entry.insert("parts".into(), serde_json::to_value(parts)?);
            required.insert(id, source_refs);
            prompt.push(Value::Object(entry));
        }
        Ok(Self {
            prompt: Value::Array(prompt),
            references,
            required,
        })
    }

    pub(super) fn prompt_transcript(&self) -> Value {
        self.prompt.clone()
    }
    pub(super) fn legend(&self) -> Value {
        Value::Array(self.references.iter().enumerate().map(|(index, reference)| {
            json!({"id":index + 1,"spelling":reference.spelling,"sources":reference.sources})
        }).collect())
    }

    pub(super) fn render(&self, parts: &[Part], cited: &[usize]) -> Result<String> {
        ensure!(
            !parts.is_empty() && parts.len() <= MAX_PARTS,
            "Invalid name-reference part count"
        );
        ensure!(
            serde_json::to_vec(parts)?.len() <= MAX_BODY_BYTES,
            "Name-reference raw output exceeds budget"
        );
        ensure!(
            !cited.is_empty() && cited.len() <= MAX_SOURCES,
            "Invalid name-reference citation count"
        );
        let cited_set: BTreeSet<_> = cited.iter().copied().collect();
        ensure!(
            cited_set.len() == cited.len()
                && cited_set.iter().all(|id| self.required.contains_key(id)),
            "Unknown or duplicate name-reference source"
        );
        let mut output = String::new();
        let mut copies = Vec::new();
        let mut used = BTreeSet::new();
        let mut prose = false;
        for part in parts {
            let start = output.len();
            match (&part.t, part.n) {
                (Some(text), None) => {
                    ensure!(!text.is_empty(), "Empty name-reference prose fragment");
                    prose |= text.chars().any(char::is_alphanumeric);
                    output.push_str(text);
                }
                (None, Some(id)) => {
                    let reference = id
                        .checked_sub(1)
                        .and_then(|index| self.references.get(index))
                        .context("Unknown name reference")?;
                    ensure!(
                        reference
                            .sources
                            .iter()
                            .any(|source| cited_set.contains(source)),
                        "Name reference belongs to another source"
                    );
                    output.push_str(&reference.spelling);
                    copies.push((start..output.len(), id));
                    used.insert(id);
                }
                _ => bail!("A name-reference part must have exactly one variant"),
            }
            ensure!(
                output.len() <= MAX_BODY_BYTES,
                "Name-reference rendered output exceeds budget"
            );
        }
        ensure!(prose, "Name-reference output omitted surrounding prose");
        ensure!(
            cited_set
                .iter()
                .all(|source| self.required[source].is_subset(&used)),
            "Name-reference output omitted a cited source spelling"
        );
        for (range, id) in &copies {
            ensure!(
                boundary(&output, range, &self.references[id - 1].spelling),
                "Name-reference output extended a source spelling"
            );
        }
        // Inspect the assembled string too: splitting one literal name across
        // adjacent t parts must not bypass the reference contract.
        for reference in &self.references {
            for range in spans(&output, &reference.spelling) {
                ensure!(
                    copies
                        .iter()
                        .any(|(copied, _)| copied.start <= range.start && range.end <= copied.end),
                    "Literal prose rewrote a name reference"
                );
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(value: Value) -> Vec<Part> {
        serde_json::from_value(value).unwrap()
    }
    fn reference(bindings: &Bindings, spelling: &str) -> usize {
        bindings
            .legend()
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["spelling"] == spelling)
            .unwrap()["id"]
            .as_u64()
            .unwrap() as usize
    }
    fn declared(words: &[&str]) -> Terms {
        Terms::new(
            &[],
            &words
                .iter()
                .map(|word| crate::library::Entry {
                    kind: "term".into(),
                    title: (*word).into(),
                    enabled: true,
                    ..Default::default()
                })
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn utf8_spans_round_trip_exactly_and_preserve_typed_metadata() -> Result<()> {
        let text = "刘老师继续负责布置，使用OmaScribe进行转写。";
        let input = json!({"transcript":[{"id":3,"text":text,"speaker":"如果","revision":9,"metadata":{"verified":false}}]});
        let bindings = Bindings::new(&input, &declared(&["刘老师", "OmaScribe"]))?;
        assert_eq!(bindings.legend().as_array().unwrap().len(), 2);
        let prompt = bindings.prompt_transcript();
        assert!(prompt[0].get("text").is_none());
        assert_eq!(prompt[0]["id"], 3);
        assert_eq!(prompt[0]["speaker"], "如果");
        assert_eq!(prompt[0]["revision"], 9);
        assert_eq!(prompt[0]["metadata"], input["transcript"][0]["metadata"]);
        assert_eq!(
            bindings.render(&parts(prompt[0]["parts"].clone()), &[3])?,
            text
        );
        let serialized = serde_json::to_string(&parts(prompt[0]["parts"].clone()))?;
        assert!(!serialized.contains("null"));
        Ok(())
    }

    #[test]
    fn identical_spellings_share_only_literal_provenance_and_keep_actual_source_ids() -> Result<()>
    {
        let input = json!({"transcript":[
            {"id":1,"text":"ALICE WILL CHECK THE REPORT"},
            {"id":3,"text":"ALICE WILL VERIFY THE EXPORT"},
            {"id":5,"text":"BOB WILL CHECK THE RECORDING"}
        ]});
        let bindings = Bindings::new(&input, &Terms::new(&[], &[]))?;
        let alice = reference(&bindings, "ALICE");
        let bob = reference(&bindings, "BOB");
        assert_eq!(bindings.legend()[alice - 1]["sources"], json!([1, 3]));
        assert_eq!(bindings.legend()[bob - 1]["sources"], json!([5]));
        // This is membership of a spelling, not an assertion of one person's identity.
        assert_eq!(
            bindings.render(
                &parts(json!([{"n":alice},{"t":" will check the report."}])),
                &[1, 3]
            )?,
            "ALICE will check the report."
        );
        assert!(bindings
            .render(
                &parts(json!([{"n":bob},{"t":" will check the report."}])),
                &[1]
            )
            .is_err());
        assert!(bindings
            .render(
                &parts(json!([{"n":alice},{"t":" will check the report."}])),
                &[5]
            )
            .is_err());
        assert!(bindings
            .render(
                &parts(json!([{"n":alice},{"t":" will check the report."}])),
                &[1, 5]
            )
            .is_err());
        Ok(())
    }

    #[test]
    fn asr_variants_are_distinct_spellings_and_never_fuzzy_identity_merges() -> Result<()> {
        let input = json!({"transcript":[
            {"id":1,"text":"当迪大学的 PAMELA FERGUSON教授说这可能尚未完成。"},
            {"id":4,"text":"当迪大学的 PAMELA FERGUSTON教授说仍需要确认。"}
        ]});
        let bindings = Bindings::new(&input, &Terms::new(&[], &[]))?;
        let ferguson = reference(&bindings, "PAMELA FERGUSON");
        let ferguston = reference(&bindings, "PAMELA FERGUSTON");
        assert_ne!(ferguson, ferguston);
        assert_eq!(bindings.legend()[ferguson - 1]["sources"], json!([1]));
        assert_eq!(bindings.legend()[ferguston - 1]["sources"], json!([4]));
        let prompt = bindings.prompt_transcript();
        assert_eq!(
            bindings.render(&parts(prompt[0]["parts"].clone()), &[1])?,
            input["transcript"][0]["text"].as_str().unwrap()
        );
        assert_eq!(
            bindings.render(&parts(prompt[1]["parts"].clone()), &[4])?,
            input["transcript"][1]["text"].as_str().unwrap()
        );
        Ok(())
    }

    #[test]
    fn a_short_spelling_inside_a_longer_reference_does_not_merge_their_sources() -> Result<()> {
        let terms = declared(&["ALICE", "ALICE SMITH"]);
        let input = json!({"transcript":[
            {"id":2,"text":"ALICE will review the report."},
            {"id":6,"text":"ALICE SMITH will check the export."}
        ]});
        let bindings = Bindings::new(&input, &terms)?;
        let long = reference(&bindings, "ALICE SMITH");
        let value = parts(json!([{"n":long},{"t":" will check the export."}]));
        assert_eq!(
            bindings.render(&value, &[6])?,
            "ALICE SMITH will check the export."
        );
        assert!(bindings.render(&value, &[2]).is_err());
        assert!(bindings.render(&value, &[2, 6]).is_err());
        Ok(())
    }

    #[test]
    fn names_that_look_like_conditions_remain_literal_bytes_not_scope_permissions() -> Result<()> {
        let input = json!({"transcript":[{"id":7,"text":"If remains the tool name; 如果仍是标签。","speaker":"If"}]});
        let bindings = Bindings::new(&input, &declared(&["If", "如果"]))?;
        assert_eq!(bindings.legend().as_array().unwrap().len(), 2);
        let prompt = bindings.prompt_transcript();
        assert_eq!(
            bindings.render(&parts(prompt[0]["parts"].clone()), &[7])?,
            input["transcript"][0]["text"].as_str().unwrap()
        );
        assert_eq!(prompt[0]["speaker"], "If");
        Ok(())
    }

    #[test]
    fn every_cited_active_name_needs_a_reference_and_literals_cannot_bypass_it() -> Result<()> {
        let bindings = Bindings::new(
            &json!({"transcript":[{"id":2,"text":"ALICE WILL CHECK THE REPORT"}]}),
            &Terms::new(&[], &[]),
        )?;
        let alice = reference(&bindings, "ALICE");
        for value in [
            json!([{"t":"The report still needs review."}]),
            json!([{"t":"ALICE will check the report."}]),
            json!([{"n":alice},{"t":" and ALICE will check the report."}]),
            json!([{"n":alice},{"t":" and AL"},{"t":"ICE will check the report."}]),
            json!([{"n":alice},{"t":"X will check the report."}]),
            json!([{"t":"X"},{"n":alice},{"t":" will check the report."}]),
            json!([{"n":alice},{"t":"."}]),
        ] {
            assert!(
                bindings.render(&parts(value.clone()), &[2]).is_err(),
                "{value}"
            );
        }
        assert_eq!(
            bindings.render(
                &parts(json!([{"n":alice},{"t":" will check the report."}])),
                &[2]
            )?,
            "ALICE will check the report."
        );
        Ok(())
    }

    #[test]
    fn inactive_unspoken_names_are_not_required_or_inferred_from_word_fragments() -> Result<()> {
        let terms = declared(&["ALICE", "HistoricalRobot", "OmaScribe"]);
        let bindings = Bindings::new(
            &json!({"transcript":[{"id":9,"text":"The new date remains undecided."}]}),
            &terms,
        )?;
        assert_eq!(bindings.legend(), json!([]));
        assert_eq!(
            bindings.render(
                &parts(json!([{"t":"The new date remains undecided."}])),
                &[9]
            )?,
            "The new date remains undecided."
        );
        let fragments = Bindings::new(
            &json!({"transcript":[{"id":10,"text":"A L I C E remains unresolved."}]}),
            &terms,
        )?;
        assert!(!fragments.legend().to_string().contains("ALICE"));
        let embedded = Bindings::new(
            &json!({"transcript":[{"id":11,"text":"MYOMASCRIBER and XALICEY remain unresolved."}]}),
            &terms,
        )?;
        assert!(!embedded
            .legend()
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["spelling"] == "OmaScribe" || entry["spelling"] == "ALICE"));
        Ok(())
    }

    #[test]
    fn malformed_variants_unknown_refs_and_unknown_citations_are_rejected() -> Result<()> {
        for raw in [
            r#"[{"t":null}]"#,
            r#"[{"n":null}]"#,
            r#"[{"t":null,"n":1}]"#,
            r#"[{"n":-1}]"#,
            r#"[{"n":1.5}]"#,
            r#"[{"t":"a","t":"b"}]"#,
            r#"[{"t":"a","extra":1}]"#,
        ] {
            assert!(serde_json::from_str::<Vec<Part>>(raw).is_err(), "{raw}");
        }
        let bindings = Bindings::new(
            &json!({"transcript":[{"id":3,"text":"ALICE WILL CHECK THE REPORT"}]}),
            &Terms::new(&[], &[]),
        )?;
        for value in [
            json!([]),
            json!([{}]),
            json!([{"t":""}]),
            json!([{"t":"text","n":1}]),
            json!([{"n":0},{"t":" will check."}]),
            json!([{"n":49},{"t":" will check."}]),
        ] {
            assert!(
                bindings.render(&parts(value.clone()), &[3]).is_err(),
                "{value}"
            );
        }
        let valid = parts(json!([{"n":1},{"t":" will check the report."}]));
        for cited in [vec![], vec![0], vec![1], vec![3, 3], vec![3, 4]] {
            assert!(bindings.render(&valid, &cited).is_err(), "{cited:?}");
        }
        for input in [
            json!({}),
            json!({"transcript":[]}),
            json!({"transcript":[{"id":0,"text":"text"}]}),
            json!({"transcript":[{"id":1}]}),
            json!({"transcript":[{"id":1,"text":"text","parts":[]}]}),
            json!({"transcript":[{"id":1,"text":"text"},{"id":1,"text":"text"}]}),
        ] {
            assert!(
                Bindings::new(&input, &Terms::new(&[], &[])).is_err(),
                "{input}"
            );
        }
        Ok(())
    }

    #[test]
    fn hard_budgets_reject_large_inputs_reference_sets_and_part_outputs() -> Result<()> {
        let terms = Terms::new(&[], &[]);
        assert!(Bindings::new(
            &json!({"transcript":[{"id":1,"text":"x".repeat(MAX_INPUT_BYTES)}]}),
            &terms
        )
        .is_err());
        let transcript: Vec<_> = (1..=MAX_REFERENCES + 1)
            .map(|id| json!({"id":id,"text":format!("R{id:03} WILL CHECK THE REPORT")}))
            .collect();
        assert!(Bindings::new(&json!({"transcript":transcript}), &terms).is_err());
        assert!(Bindings::new(&json!({"transcript":[{"id":1,"text":format!("{}WILL CHECK THE REPORT", "ALICE ".repeat(MAX_PARTS))}]}), &terms).is_err());
        let bindings = Bindings::new(
            &json!({"transcript":[{"id":1,"text":"The review remains pending."}]}),
            &terms,
        )?;
        let too_many = (0..=MAX_PARTS)
            .map(|_| Part {
                t: Some("x".into()),
                n: None,
            })
            .collect::<Vec<_>>();
        assert!(bindings.render(&too_many, &[1]).is_err());
        assert!(bindings
            .render(
                &[Part {
                    t: Some("x".repeat(MAX_BODY_BYTES)),
                    n: None
                }],
                &[1]
            )
            .is_err());
        Ok(())
    }
}
