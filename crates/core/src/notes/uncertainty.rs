//! Restore an omitted hedge only for the same literal subject + claim from the
//! latest source. Do not guess paraphrases, identities, or cross-run state.
use crate::Segment;
use std::collections::BTreeMap;

fn compact(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}
pub(super) fn restore(body: &str, segments: &[Segment]) -> String {
    let Some(first) = segments.first() else {
        return body.to_owned();
    };
    if segments
        .iter()
        .any(|s| s.meeting_id != first.meeting_id || s.run != first.run)
    {
        return body.to_owned();
    }
    let mut text = String::new();
    let mut offsets = BTreeMap::new();
    for (at, c) in body.char_indices().filter(|(_, c)| !c.is_whitespace()) {
        offsets.insert(text.len(), at);
        text.push(c);
    }
    let mut insertions = BTreeMap::new();
    for s in segments {
        let clauses = s
            .text
            .split(['，', '。', '；', '？', '！', ',', '.', ';', '?', '!', '\n'])
            .collect::<Vec<_>>();
        for (index, clause) in clauses.iter().enumerate() {
            let clause = compact(clause);
            if ["如果", "假如", "是否"]
                .iter()
                .any(|p| clause.starts_with(p))
            {
                continue;
            }
            for hedge in ["好像", "似乎", "可能", "疑似"] {
                let Some((subject, claim)) = clause.split_once(hedge) else {
                    continue;
                };
                let subject = subject
                    .strip_prefix("这个")
                    .or_else(|| subject.strip_prefix("那个"))
                    .unwrap_or(subject);
                if subject.chars().count() < 2 || claim.chars().count() < 2 {
                    continue;
                }
                let updates = |text: &str| {
                    compact(text).contains(subject)
                        || [
                            "确认",
                            "确定",
                            "证实",
                            "核实",
                            "不再",
                            "更正",
                            "改为",
                            "已修复",
                        ]
                        .iter()
                        .any(|cue| text.contains(cue))
                };
                if clauses[index + 1..].iter().any(|text| updates(text))
                    || segments
                        .iter()
                        .any(|later| later.seq > s.seq && updates(&later.text))
                {
                    continue;
                }
                let assertion = format!("{subject}{claim}");
                for (at, _) in text.match_indices(&assertion) {
                    if let Some(original) = offsets.get(&(at + subject.len())) {
                        insertions.insert(*original, hedge);
                    }
                }
            }
        }
    }
    let mut result = body.to_owned();
    for (at, hedge) in insertions.into_iter().rev() {
        result.insert_str(at, hedge);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(seq: u32, text: &str) -> Segment {
        Segment {
            id: seq.to_string(),
            meeting_id: "m".into(),
            run: 1,
            seq,
            start_ms: 0,
            end_ms: 1,
            text: text.into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        }
    }
    #[test]
    fn omitted_hedge_returns_only_to_its_own_literal_claim() {
        let sources = [
            source(1, "这个 PixelLedger 的人员分配好像有问题，转写还需要核对。"),
            source(2, "OtherLedger的人员分配有问题。"),
        ];
        let body = "转写中提到 PixelLedger 的人员分配有问题。OtherLedger的人员分配有问题。";
        assert_eq!(
            restore(body, &sources),
            "转写中提到 PixelLedger 的人员分配好像有问题。OtherLedger的人员分配有问题。"
        );
        let hedged = "PixelLedger 的人员分配似乎有问题。";
        assert_eq!(restore(hedged, &sources), hedged);
        assert_eq!(
            restore("PixelLedger的人员分配没有问题。", &sources),
            "PixelLedger的人员分配没有问题。"
        );
    }
    #[test]
    fn newer_mentions_changed_runs_and_paraphrases_are_never_repaired_by_guessing() {
        let body = "PixelLedger的人员分配有问题。";
        let mut sources = vec![
            source(1, "PixelLedger的人员分配可能有问题。"),
            source(2, "确认PixelLedger的人员分配有问题。"),
        ];
        assert_eq!(restore(body, &sources), body);
        sources[1].run = 2;
        assert_eq!(restore(body, &sources), body);
        let body = "PixelLedger存在人员分配问题。";
        assert_eq!(restore(body, &sources[..1]), body);
        let body = "PixelLedger的人员分配有问题。";
        assert_eq!(
            restore(
                body,
                &[source(
                    1,
                    "PixelLedger的人员分配可能有问题。确认PixelLedger的人员分配有问题。"
                )]
            ),
            body
        );
    }
}
