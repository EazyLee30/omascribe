//! A narrow conservation check for explicit, positive local recording retention.
//! Ambiguous records, conditions and changed destinations are not reconstructed.

use std::{fmt, sync::OnceLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Mismatch {
    RecordingOrLocalDestination,
    InNotesRelation,
    CoordinatedNotesRelation,
}

impl Mismatch {
    pub(crate) fn hint(&self) -> &'static str {
        match self {
            Self::RecordingOrLocalDestination => {
                "Translate the ORIGINAL source again. Preserve its explicit recording/audio object and local-retention requirement together, rather than retaining meeting notes alone or borrowing a location from unrelated settings. Where the original explicitly says recording IN meeting notes, preserve containment rather than AND; where it explicitly says AND, preserve both coordinated objects. Keep all other clauses, negation and dates. Do not add words directly to the rejected draft or guess a missing conjunction. 原话明确的录音对象和本机保留要求须同时保留，不可只剩会议笔记；原话的包含与并列关系不可互换，不要补写含混关系。"
            }
            Self::InNotesRelation => {
                "Translate the ORIGINAL source again. Its recording is IN meeting notes: preserve that containment relation and local-retention requirement. Do not silently replace IN with AND, invent two coordinated objects, or resolve malformed speech by guessing. Preserve the other source clauses and dates. 保留原话中录音在会议笔记中的关系，不擅自改成录音和会议笔记；含混原话保持含混。"
            }
            Self::CoordinatedNotesRelation => {
                "Translate the ORIGINAL source again. It explicitly coordinates recordings AND meeting notes. Preserve both objects and their local-retention requirement, without replacing the supplied conjunction with containment. Keep every other source clause and date. 原话明确并列录音和会议笔记，两项及本机保留要求都须保留，不改成包含关系。"
            }
        }
    }
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RecordingOrLocalDestination => "译文遗漏原话的录音对象或本机保留限定",
            Self::InNotesRelation => "译文改写了原话录音在会议笔记中的关系",
            Self::CoordinatedNotesRelation => "译文遗漏或改写了录音和会议笔记的并列保留关系",
        })
    }
}

impl std::error::Error for Mismatch {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Relation {
    Unspecified,
    InNotes,
    AndNotes,
    Ambiguous,
}

fn re(cell: &'static OnceLock<regex::Regex>, pattern: &str) -> &'static regex::Regex {
    cell.get_or_init(|| regex::Regex::new(pattern).unwrap())
}

fn sentences(text: &str) -> impl Iterator<Item = &str> {
    text.split(['.', '!', '?', ';', '\n', '。', '！', '？', '；'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn clauses(text: &str) -> impl Iterator<Item = &str> {
    sentences(text)
        .flat_map(|s| s.split([',', '，']))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn object(text: &str, source: bool) -> Option<regex::Match<'_>> {
    static SOURCE: OnceLock<regex::Regex> = OnceLock::new();
    static BODY: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = if source {
        re(
            &SOURCE,
            r"(?i)\b(?:audio\s+)?recordings?\b|录音(?:文件|内容)?|錄音(?:檔案|內容)?",
        )
    } else {
        re(
            &BODY,
            r"(?i)\b(?:(?:audio\s+)?recordings?|recorded\s+audio|audio\s+files?)\b|录音(?:文件|内容)?|錄音(?:檔案|內容)?|录制内容|音频文件|音訊檔案",
        )
    };
    pattern.find(text)
}

fn non_audio_label(text: &str, audio: regex::Match<'_>) -> bool {
    static TAIL: OnceLock<regex::Regex> = OnceLock::new();
    static LABEL: OnceLock<regex::Regex> = OnceLock::new();
    re(&TAIL, r"(?i)^\s+(?:(?:a|the|of)\s+)?(?:decisions?|transactions?|votes?|attendance|events?|meeting\s+notes|buttons?|icons?|labels?|indicators?)\b")
        .is_match(&text[audio.end()..])
        || re(&LABEL, r#"(?i)\b(?:project|product|button|label|called|named)\s+["'“‘]?\s*$"#)
            .is_match(&text[..audio.start()])
}

fn abstains(source: &str) -> bool {
    static NEGATED: OnceLock<regex::Regex> = OnceLock::new();
    static CONDITIONAL: OnceLock<regex::Regex> = OnceLock::new();
    static CHANGED: OnceLock<regex::Regex> = OnceLock::new();
    // Negation belongs to the retention action, not an unrelated prefix such
    // as FRIDAY IS NOT THE DEADLINE in the retained malformed ASR source.
    sentences(source).any(|clause| {
        let Some(audio) = object(clause, true) else {
            return false;
        };
        if non_audio_label(clause, audio) {
            return false;
        }
        re(&NEGATED, r"(?i)\b(?:not|never|no\s+longer|don't|mustn't|shouldn't)\s+(?:be\s+)?(?:stay|remain|keep|kept|store|stored|save|saved|retain|retained)\b|\bno\s+(?:audio\s+)?recordings?\b|\bnot\s+(?:on\s+this\s+(?:computer|device)|locally)\b|(?:不再|不能|不得|无需|不应|不要|不)(?:在(?:本机|本地))?(?:保留|保存|存储|留存|存放|留)")
            .is_match(clause)
            || re(&CONDITIONAL, r"(?i)\b(?:if|unless|provided|conditional|hypothetically)\b|\b(?:after|once)\s+(?:approval|approved)\b|如果|假如|若|获批后|批准后|只有.*才")
                .is_match(clause)
            || re(&CHANGED, r"(?i)\b(?:instead|cloud|remote\s+(?:server|storage)|moved?|switched?|changed?|cancelled?|canceled?)\b|改为|改存|转存|迁移|撤回|取消|云端|雲端|远程存储")
                .is_match(clause)
    })
}

fn local_action(text: &str) -> Option<regex::Match<'_>> {
    static ACTION: OnceLock<regex::Regex> = OnceLock::new();
    re(&ACTION, r"(?i)\b(?:stay|remain|keep|kept|store|stored|save|saved|retain|retained)\b[^,;.!?。\n，；！？]{0,80}(?:\bon\s+this\s+(?:computer|device|machine)\b|\blocally\b|\bon[- ]device\b)|\b(?:locally|on[- ]device)\s+(?:keep|kept|store|stored|save|saved|retain|retained)\b|(?:保留|保存|存储|儲存|留存|存放|留)[^,;.!?。\n，；！？]{0,40}(?:在|于)(?:本机|本機|本地|这台电脑|這台電腦|此电脑|此電腦|这台设备|這台設備|此设备|此設備)|(?:在)?(?:本机|本機|本地|这台电脑|這台電腦|此电脑|此電腦|这台设备|這台設備)(?:上)?\s*(?:保留|保存|存储|儲存|留存|存放)")
        .find(text)
}

fn relation(text: &str) -> Relation {
    static IN: OnceLock<regex::Regex> = OnceLock::new();
    static AND: OnceLock<regex::Regex> = OnceLock::new();
    let inside = re(&IN, r"(?i)\brecordings?\s+(?:in|within|inside)\s+(?:the\s+)?meeting\s+notes\b|(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)(?:中|内|內|里|裡)(?:的)?录音|录音[^,;.!?。\n，；！？]{0,35}在[^,;.!?。\n，；！？]{0,20}(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)(?:中|内|內|里|裡)")
        .is_match(text);
    let coordinated = re(&AND, r"(?i)\brecordings?\s+(?:and|&)\s+(?:the\s+)?meeting\s+notes\b|\bmeeting\s+notes\s+(?:and|&)\s+(?:the\s+)?recordings?\b|录音(?:和|与|與|及|、)(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)|(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)(?:和|与|與|及|、)录音")
        .is_match(text);
    match (inside, coordinated) {
        (true, false) => Relation::InNotes,
        (false, true) => Relation::AndNotes,
        (false, false) => Relation::Unspecified,
        (true, true) => Relation::Ambiguous,
    }
}

fn frame(clause: &str, source: bool) -> Option<Relation> {
    let audio = object(clause, source)?;
    if non_audio_label(clause, audio) || abstains(clause) {
        return None;
    }
    let action = local_action(clause)?;
    // Keep the object and the local-retention predicate in one bounded clause.
    // A generic 本机设置 elsewhere in a section cannot satisfy this check.
    if audio.start().abs_diff(action.start()) > 120 {
        return None;
    }
    static AFTER_OBJECT: OnceLock<regex::Regex> = OnceLock::new();
    static BEFORE_OBJECT: OnceLock<regex::Regex> = OnceLock::new();
    if action.start() >= audio.end() {
        let bridge = &clause[audio.end()..action.start()];
        if !re(&AFTER_OBJECT, r"(?ix)^\s*(?:(?:(?:in|within|inside|and|&)\s+(?:the\s+)?meeting\s+notes\s+)?(?:(?:should|must|shall|will|needs?\s+to|ought\s+to)\s+)?(?:be\s+)?|(?:(?:在(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)(?:中|内|內|里|裡))|(?:(?:和|与|與|及|、)(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)))?(?:应|应该|必须|须|需|需要|只|仅|仍|继续|将|会|要|被|\s)*)$")
            .is_match(bridge)
        {
            return None;
        }
    } else {
        let bridge = &clause[action.start()..audio.start()];
        if !re(&BEFORE_OBJECT, r"(?ix)^(?:(?:(?:locally|on[- ]device)\s+)?(?:keep|kept|store|stored|save|saved|retain|retained)\s+(?:(?:the|this|these|original)\s+)*|(?:(?:在)?(?:本机|本機|本地|这台电脑|這台電腦|此电脑|此電腦|这台设备|這台設備)(?:上)?\s*)?(?:保留|保存|存储|儲存|留存|存放|留)(?:(?:在|于)(?:本机|本機|本地|这台电脑|這台電腦|此电脑|此電腦|这台设备|這台設備)(?:上)?)?(?:的)?\s*)$")
            .is_match(bridge)
        {
            return None;
        }
    }
    Some(relation(clause))
}

pub(crate) fn validate<'a>(
    body: &str,
    sources: impl IntoIterator<Item = &'a str>,
) -> Result<(), Mismatch> {
    let sources: Vec<_> = sources.into_iter().collect();
    // A clearly changed/conditional/negative retention source is not resolved
    // by borrowing a positive destination from another selected source.
    if sources.iter().any(|s| abstains(s)) {
        return Ok(());
    }
    let required: Vec<_> = sources
        .iter()
        .flat_map(|source| clauses(source))
        .filter_map(|clause| frame(clause, true))
        .collect();
    if required.is_empty()
        || required.contains(&Relation::Ambiguous)
        || required.iter().any(|r| *r != required[0])
    {
        return Ok(());
    }
    let actual: Vec<_> = clauses(body)
        .filter_map(|clause| frame(clause, false))
        .collect();
    if actual.is_empty() {
        return Err(Mismatch::RecordingOrLocalDestination);
    }
    if required[0] != Relation::Unspecified && !actual.contains(&required[0]) {
        return Err(if required[0] == Relation::AndNotes {
            Mismatch::CoordinatedNotesRelation
        } else {
            Mismatch::InNotesRelation
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MALFORMED: &str =
        "FRIDAY IS NOT THE DEADLINE RECORDING IN MEETING NOTES SHOULD STAY ON THIS COMPUTER";

    #[test]
    fn original_missing_recording_outputs_are_rejected_despite_unrelated_not() {
        for body in [
            "星期五不是截止日期，会议记录应留在此电脑上",
            "星期五不是截止日期，会议纪要应留在此电脑上",
        ] {
            assert_eq!(
                validate(body, [MALFORMED]),
                Err(Mismatch::RecordingOrLocalDestination)
            );
        }
        assert!(validate(
            "星期五不是截止日期，会议笔记中的录音应保留在此电脑上。",
            [MALFORMED]
        )
        .is_ok());
    }

    #[test]
    fn explicit_in_is_not_guessed_as_and() {
        assert_eq!(
            validate("录音和会议笔记应保留在本机。", [MALFORMED]),
            Err(Mismatch::InNotesRelation)
        );
        assert_eq!(
            validate(
                "Recordings and meeting notes should stay on this computer.",
                [MALFORMED]
            ),
            Err(Mismatch::InNotesRelation)
        );
        assert!(validate(
            "The recording in meeting notes should remain on this computer.",
            [MALFORMED]
        )
        .is_ok());
        assert!(validate("录音在会议笔记中应保留在本机。", [MALFORMED]).is_ok());
    }

    #[test]
    fn normal_audio_retention_and_explicit_coordination_are_supported() {
        for (source, body) in [
            (
                "The recording should stay on this computer.",
                "录音应保留在本机。",
            ),
            (
                "We must keep the audio recording on this device.",
                "录音必须保存在这台设备上。",
            ),
            ("录音应保留在本机。", "Keep the recording on this computer."),
            (
                "Recordings and meeting notes should remain on this computer.",
                "录音和会议笔记应保留在本机。",
            ),
        ] {
            assert!(validate(body, [source]).is_ok(), "{source} -> {body}");
        }
        let error = validate(
            "录音应保留在本机。",
            ["Recordings and meeting notes should remain on this computer."],
        )
        .unwrap_err();
        assert_eq!(error, Mismatch::CoordinatedNotesRelation);
        assert!(error.hint().contains("AND meeting notes"));
    }

    #[test]
    fn missing_destination_or_unrelated_local_settings_cannot_satisfy_recording() {
        let source = "The recording should stay on this computer.";
        for body in [
            "录音应保留。",
            "录音已准备好。本机设置已保存。",
            "录音用于参考，本地保存会议笔记。",
            "The recording is ready. Keep the settings on this computer.",
            "The recording is ready and keep the settings on this computer.",
        ] {
            assert!(validate(body, [source]).is_err(), "{body}");
        }
    }

    #[test]
    fn separate_sources_do_not_synthesize_a_retention_statement() {
        assert!(validate(
            "会议笔记保存在本机。",
            [
                "The recording is ready.",
                "Meeting notes should stay on this computer."
            ]
        )
        .is_ok());
        assert!(validate(
            "会议笔记保存在本机。",
            ["Recording", "should stay on this computer"]
        )
        .is_ok());
    }

    #[test]
    fn notes_only_labels_and_non_audio_record_keeping_are_not_forced() {
        for source in [
            "Meeting notes should stay on this computer.",
            "Project Recording should stay on this computer.",
            "Recording decisions in meeting notes should stay on this computer.",
            "The recording button should stay on this computer.",
            "Record all decisions in meeting notes; keep notes locally.",
        ] {
            assert!(
                validate("会议笔记保存在本机。", [source]).is_ok(),
                "{source}"
            );
        }
    }

    #[test]
    fn negated_conditional_changed_or_conflicting_retention_abstains() {
        for source in [
            "The recording should not stay on this computer.",
            "Do not keep the recording on this computer.",
            "If approved the recording should stay on this computer.",
            "If approved, the recording should stay on this computer.",
            "The recording should stay on this computer if approved.",
            "The recording was moved to cloud storage instead.",
            "录音不要保存在本机。",
            "如果获批录音保存在本机。",
            "录音改存云端。",
        ] {
            assert!(
                validate("会议笔记保存在本机。", [source]).is_ok(),
                "{source}"
            );
        }
        assert!(validate(
            "会议笔记保存在本机。",
            [
                "The recording should stay on this computer.",
                "The recording was moved to cloud storage instead."
            ]
        )
        .is_ok());
    }
}
