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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Anchor {
    pub text: String,
    pub required_object_phrase: &'static str,
    pub source_span: std::ops::Range<usize>,
    pub remaining_source: String,
    pub has_other_context: bool,
}

#[derive(Clone, Copy)]
enum Modality {
    Should,
    Must,
    Shall,
    Need,
    Command,
}

#[derive(Clone, Copy)]
enum Destination {
    Computer,
    Device,
    Machine,
    Local,
}

fn copular_context(text: &str) -> bool {
    // Only this bounded complete copular prefix is admitted without a source
    // sentence boundary. An arbitrary adjective before "recording" is not
    // consumed or mistaken for an independent statement.
    static COPULAR: OnceLock<regex::Regex> = OnceLock::new();
    re(&COPULAR, r"(?i)^[a-z][a-z0-9 -]{0,60}\s+(?:is|are|was|were)\s+not\s+(?:the|a|an)\s+(?:deadline|due\s+date|release\s+date|owner|budget)[,:]?$")
        .is_match(text)
}

fn independent_prefix(prefix: &str) -> bool {
    let trimmed = prefix.trim_end();
    trimmed.is_empty()
        || prefix.ends_with('\n')
        || trimmed.ends_with(['.', ';', '。', '；', '?', '？', '!', '！'])
        || copular_context(
            trimmed
                .rsplit(['.', ';', '。', '；', '?', '？', '!', '！', '\n'])
                .next()
                .unwrap_or("")
                .trim(),
        )
}

fn independent_suffix(suffix: &str) -> bool {
    // A source boundary remains meaningful after horizontal whitespace.
    // trim_start() would erase the newline before testing that boundary.
    let text = suffix.trim_start_matches([' ', '\t', '\r']);
    if text.is_empty() || text.starts_with(['.', ';', '。', '；', '\n']) {
        return true;
    }
    let remaining = text
        .strip_prefix(',')
        .or_else(|| text.strip_prefix('，'))
        .or_else(|| text.strip_prefix("and "))
        .or_else(|| text.strip_prefix("AND "));
    remaining.is_some_and(|s| copular_context(s.trim().trim_end_matches(['.', '。'])))
}

fn object_phrase(relation: Relation, chinese: bool, audio: bool, plural: bool) -> &'static str {
    if chinese {
        return match relation {
            Relation::InNotes => "会议笔记中的录音",
            Relation::AndNotes => "录音和会议笔记",
            _ => "录音",
        };
    }
    match (relation, audio, plural) {
        (Relation::InNotes, true, true) => "audio recordings in meeting notes",
        (Relation::InNotes, true, false) => "audio recording in meeting notes",
        (Relation::InNotes, false, true) => "recordings in meeting notes",
        (Relation::InNotes, false, false) => "recording in meeting notes",
        (Relation::AndNotes, true, true) => "audio recordings and meeting notes",
        (Relation::AndNotes, true, false) => "audio recording and meeting notes",
        (Relation::AndNotes, false, true) => "recordings and meeting notes",
        (Relation::AndNotes, false, false) => "recording and meeting notes",
        (_, true, true) => "audio recordings",
        (_, true, false) => "audio recording",
        (_, false, true) => "recordings",
        (_, false, false) => "recording",
    }
}

/// Decode only a complete original weekday exclusion, never an arbitrary tail.
pub(crate) fn negative_deadline(source: &str, chinese: bool) -> Option<String> {
    static DEADLINE: OnceLock<regex::Regex> = OnceLock::new();
    let captures = re(
        &DEADLINE,
        r"(?i)^(?:(?P<week>this|next)[ \t]+)?(?P<day>monday|tuesday|wednesday|thursday|friday|saturday|sunday)[ \t]+is[ \t]+not[ \t]+the[ \t]+deadline[ \t]*[.。]?$",
    )
    .captures(source.trim())?;
    let days = [
        ("Monday", "一"),
        ("Tuesday", "二"),
        ("Wednesday", "三"),
        ("Thursday", "四"),
        ("Friday", "五"),
        ("Saturday", "六"),
        ("Sunday", "日"),
    ];
    let (english, day) = days
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(&captures["day"]))?;
    let week = captures
        .name("week")
        .map(|m| m.as_str().to_ascii_lowercase());
    let (zh_prefix, en_prefix) = match week.as_deref() {
        None => ("星期", ""),
        Some("this") => ("本周", "This "),
        Some("next") => ("下周", "Next "),
        _ => return None,
    };
    Some(if chinese {
        format!("{zh_prefix}{day}不是截止日期。")
    } else {
        format!("{en_prefix}{english} is not the deadline.")
    })
}

/// Consume only a completely identified source frame. The parent translates
/// the exact before/after slices separately, never a draft of the fixed frame.
pub(crate) fn anchor(source: &str, chinese: bool) -> Option<Anchor> {
    static POST: OnceLock<regex::Regex> = OnceLock::new();
    static PRE: OnceLock<regex::Regex> = OnceLock::new();
    static ZH: OnceLock<regex::Regex> = OnceLock::new();
    static CONTEXT_EXCLUSION: OnceLock<regex::Regex> = OnceLock::new();
    if source.contains(['"', '\'', '“', '”', '‘', '’', '`'])
        || abstains(source)
        || re(&CONTEXT_EXCLUSION, r"(?i)\b(?:if|unless|provided|conditional|hypothetically|instead|cloud|moved?|switched?|changed?|cancelled?|canceled?|remote\s+(?:server|storage)|external\s+(?:drive|storage)|usb|another\s+(?:computer|device))\b|\b(?:after|once)\s+(?:approval|approved)\b|如果|假如|若|获批后|批准后|改为|改存|转存|迁移|撤回|取消|云端|雲端")
            .is_match(source)
    {
        return None;
    }
    let patterns = [
        re(
            &POST,
            r"(?i)\b(?:the\s+)?(?P<object>(?:audio\s+)?recordings?)(?:\s+(?:in|within|inside|and|&)\s+(?:the\s+)?meeting\s+notes)?\s+(?P<modality>should|must|shall)\s+(?:stay|remain|be\s+(?:kept|stored|saved|retained))\s+(?P<destination>on\s+this\s+(?:computer|device|machine)|locally)\b",
        ),
        re(
            &PRE,
            r"(?i)\b(?:(?P<modality>should|must|shall)\s+)?(?:keep|store|save|retain)\s+(?:the\s+)?(?P<object>(?:audio\s+)?recordings?)(?:\s+(?:in|within|inside|and|&)\s+(?:the\s+)?meeting\s+notes)?\s+(?P<destination>on\s+this\s+(?:computer|device|machine)|locally)\b",
        ),
        re(
            &ZH,
            r"(?P<object>(?:(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)(?:中|内|內|里|裡)的)?录音(?:在(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)(?:中|内|內|里|裡)|(?:和|与|與|以及|及)(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要))?)(?P<modality>应该|应|必须|需要|需|须)(?:保留|保持|保存|存储|留存|存放)(?:在|于)(?P<destination>本计算机|本計算機|这台电脑|這台電腦|此电脑|此電腦|这台设备|這台設備|此设备|此設備|本机|本機|本地)(?:上)?",
        ),
    ];
    let mut candidates = patterns.iter().flat_map(|p| p.captures_iter(source));
    let captures = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    let matched = captures.get(0)?;
    let relation = frame(matched.as_str(), true)?;
    if relation == Relation::Ambiguous
        || !independent_prefix(&source[..matched.start()])
        || !independent_suffix(&source[matched.end()..])
    {
        return None;
    }
    let object = captures.name("object")?.as_str().to_lowercase();
    let modality = match captures.name("modality").map(|m| m.as_str().to_lowercase()) {
        Some(s) if s == "should" || s == "应" || s == "应该" => Modality::Should,
        Some(s) if s == "must" || s == "必须" || s == "须" => Modality::Must,
        Some(s) if s == "shall" => Modality::Shall,
        Some(s) if s == "需要" || s == "需" => Modality::Need,
        None => Modality::Command,
        _ => return None,
    };
    let destination = captures.name("destination")?.as_str().to_lowercase();
    let destination = if destination.ends_with("computer")
        || destination.contains("电脑")
        || destination.contains("電腦")
        || destination.contains("计算机")
        || destination.contains("計算機")
    {
        Destination::Computer
    } else if destination.ends_with("device")
        || destination.contains("设备")
        || destination.contains("設備")
    {
        Destination::Device
    } else if destination.ends_with("machine") || destination == "本机" || destination == "本機"
    {
        Destination::Machine
    } else if destination == "locally" || destination == "本地" {
        Destination::Local
    } else {
        return None;
    };
    let required_object_phrase = object_phrase(
        relation,
        chinese,
        object.starts_with("audio"),
        object.ends_with("recordings"),
    );
    let text = if chinese {
        let destination = match destination {
            Destination::Computer => "这台电脑上",
            Destination::Device => "此设备上",
            Destination::Machine => "本机",
            Destination::Local => "本地",
        };
        let modality = match modality {
            Modality::Should => "应",
            Modality::Must | Modality::Shall => "必须",
            Modality::Need => "需要",
            Modality::Command => "",
        };
        if captures.name("modality").is_none() {
            format!("请将{required_object_phrase}保留在{destination}。")
        } else {
            format!("{required_object_phrase}{modality}保留在{destination}。")
        }
    } else {
        let destination = match destination {
            Destination::Computer => "on this computer",
            Destination::Device => "on this device",
            Destination::Machine => "on this machine",
            Destination::Local => "locally",
        };
        let modality = match modality {
            Modality::Should => "should",
            Modality::Must => "must",
            Modality::Shall => "shall",
            Modality::Need => "needs to",
            Modality::Command => "",
        };
        if modality.is_empty() {
            format!("Keep the {required_object_phrase} {destination}.")
        } else {
            format!("The {required_object_phrase} {modality} remain {destination}.")
        }
    };
    let mut source_span = matched.range();
    let mut remaining_source = format!(
        "{}{}",
        &source[..source_span.start],
        &source[source_span.end..]
    );
    let has_other_context = remaining_source.chars().any(char::is_alphanumeric);
    if !has_other_context {
        source_span = 0..source.len();
        remaining_source.clear();
    }
    Some(Anchor {
        text,
        required_object_phrase,
        source_span,
        remaining_source,
        has_other_context,
    })
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
        re(&NEGATED, r"(?i)\b(?:not|never|no\s+longer|don't|mustn't|shouldn't)\s+(?:be\s+)?(?:stay|remain|keep|kept|store|stored|save|saved|retain|retained)\b|\bno\s+(?:audio\s+)?recordings?\b|\bnot\s+(?:on\s+this\s+(?:computer|device)|locally)\b|(?:不再|不能|不得|无需|不应|不要|不)(?:在(?:本机|本地))?(?:保留|保持|保存|存储|留存|存放|留)")
            .is_match(clause)
            || re(&CONDITIONAL, r"(?i)\b(?:if|unless|provided|conditional|hypothetically)\b|\b(?:after|once)\s+(?:approval|approved)\b|如果|假如|若|获批后|批准后|只有.*才")
                .is_match(clause)
            || re(&CHANGED, r"(?i)\b(?:instead|cloud|remote\s+(?:server|storage)|moved?|switched?|changed?|cancelled?|canceled?)\b|改为|改存|转存|迁移|撤回|取消|云端|雲端|远程存储")
                .is_match(clause)
    })
}

fn local_action(text: &str) -> Option<regex::Match<'_>> {
    static ACTION: OnceLock<regex::Regex> = OnceLock::new();
    re(&ACTION, r"(?i)\b(?:stay|remain|keep|kept|store|stored|save|saved|retain|retained)\b[^,;.!?。\n，；！？]{0,80}(?:\bon\s+this\s+(?:computer|device|machine)\b|\blocally\b|\bon[- ]device\b)|\b(?:locally|on[- ]device)\s+(?:keep|kept|store|stored|save|saved|retain|retained)\b|(?:保留|保持|保存|存储|儲存|留存|存放|留)[^,;.!?。\n，；！？]{0,40}(?:在|于)(?:本机|本機|本地|本计算机|本計算機|这台电脑|這台電腦|此电脑|此電腦|这台设备|這台設備|此设备|此設備)|(?:在)?(?:本机|本機|本地|本计算机|本計算機|这台电脑|這台電腦|此电脑|此電腦|这台设备|這台設備)(?:上)?\s*(?:保留|保持|保存|存储|儲存|留存|存放)")
        .find(text)
}

fn relation(text: &str) -> Relation {
    static IN: OnceLock<regex::Regex> = OnceLock::new();
    static AND: OnceLock<regex::Regex> = OnceLock::new();
    let inside = re(&IN, r"(?i)\brecordings?\s+(?:in|within|inside)\s+(?:the\s+)?meeting\s+notes\b|(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)(?:中|内|內|里|裡)(?:的)?录音|录音[^,;.!?。\n，；！？]{0,35}在[^,;.!?。\n，；！？]{0,20}(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)(?:中|内|內|里|裡)")
        .is_match(text);
    let coordinated = re(&AND, r"(?i)\brecordings?\s+(?:and|&)\s+(?:the\s+)?meeting\s+notes\b|\bmeeting\s+notes\s+(?:and|&)\s+(?:the\s+)?recordings?\b|录音(?:和|与|與|以及|及|、)(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)|(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)(?:和|与|與|以及|及|、)录音")
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
        if !re(&AFTER_OBJECT, r"(?ix)^\s*(?:(?:(?:in|within|inside|and|&)\s+(?:the\s+)?meeting\s+notes\s+)?(?:(?:should|must|shall|will|needs?\s+to|ought\s+to)\s+)?(?:be\s+)?|(?:(?:在(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)(?:中|内|內|里|裡))|(?:(?:和|与|與|以及|及|、)(?:会议|會議)(?:笔记|筆記|记录|記錄|纪要|紀要)))?(?:应|应该|必须|须|需|需要|只|仅|仍|继续|将|会|要|被|\s)*)$")
            .is_match(bridge)
        {
            return None;
        }
    } else {
        let bridge = &clause[action.start()..audio.start()];
        if !re(&BEFORE_OBJECT, r"(?ix)^(?:(?:(?:locally|on[- ]device)\s+)?(?:keep|kept|store|stored|save|saved|retain|retained)\s+(?:(?:the|this|these|original)\s+)*|(?:(?:在)?(?:本机|本機|本地|本计算机|本計算機|这台电脑|這台電腦|此电脑|此電腦|这台设备|這台設備)(?:上)?\s*)?(?:保留|保持|保存|存储|儲存|留存|存放|留)(?:(?:在|于)(?:本机|本機|本地|本计算机|本計算機|这台电脑|這台電腦|此电脑|此電腦|这台设备|這台設備)(?:上)?)?(?:的)?\s*)$")
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
    // A correct fixed frame must not hide a second contradictory relation.
    let conflicting = clauses(body).any(|clause| {
        let relation = relation(clause);
        match required[0] {
            Relation::InNotes => matches!(relation, Relation::AndNotes | Relation::Ambiguous),
            Relation::AndNotes => matches!(relation, Relation::InNotes | Relation::Ambiguous),
            _ => false,
        }
    });
    if conflicting || required[0] != Relation::Unspecified && !actual.contains(&required[0]) {
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
    fn negative_deadline_preserves_every_weekday_and_only_explicit_week_scope() {
        for (day, unqualified, this_week, next_week) in [
            ("Monday", "星期一", "本周一", "下周一"),
            ("Tuesday", "星期二", "本周二", "下周二"),
            ("Wednesday", "星期三", "本周三", "下周三"),
            ("Thursday", "星期四", "本周四", "下周四"),
            ("Friday", "星期五", "本周五", "下周五"),
            ("Saturday", "星期六", "本周六", "下周六"),
            ("Sunday", "星期日", "本周日", "下周日"),
        ] {
            for (scope, expected) in [
                ("", unqualified),
                ("This ", this_week),
                ("Next ", next_week),
            ] {
                let source = format!("{scope}{day} is not the deadline.");
                assert_eq!(
                    negative_deadline(&source, true),
                    Some(format!("{expected}不是截止日期。"))
                );
                assert_eq!(
                    negative_deadline(&source.to_ascii_uppercase(), false),
                    Some(source)
                );
            }
        }
        assert_eq!(
            negative_deadline("  FRIDAY IS NOT THE DEADLINE \t", true),
            Some("星期五不是截止日期。".into())
        );
        assert_eq!(
            negative_deadline("Next\tFriday IS NOT THE DEADLINE。", false),
            Some("Next Friday is not the deadline.".into())
        );
    }

    #[test]
    fn negative_deadline_does_not_consume_ambiguous_or_additional_source_speech() {
        for source in [
            MALFORMED,
            "Friday was not the deadline.",
            "If approved, Friday is not the deadline.",
            "Friday is not the deadline if approved.",
            "\"Friday is not the deadline.\"",
            "'Friday is not the deadline.'",
            "Friday is the deadline.",
            "Friday is not only the deadline.",
            "Friday is not the deadline, Saturday is.",
            "Friday is not the deadline. Keep the recording locally.",
            "Friday is not the",
            "Friday is not the deadline for testing.",
            "Last Friday is not the deadline.",
            "Upcoming Friday is not the deadline.",
            "Friday next week is not the deadline.",
            "This or next Friday is not the deadline.",
        ] {
            assert_eq!(negative_deadline(source, true), None, "{source}");
            assert_eq!(negative_deadline(source, false), None, "{source}");
        }
    }

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

    #[test]
    fn observed_keep_computer_words_are_valid_but_bare_possessive_is_not_in() {
        assert!(validate(
            "录音应保持在本计算机上。",
            ["The recording should stay on this computer."]
        )
        .is_ok());
        assert!(validate(
            "星期五不是截止日期。会议笔记中的录音应保持在本计算机上。",
            [MALFORMED]
        )
        .is_ok());
        assert_eq!(
            validate(
                "星期五不是截止日期。会议笔记的录音应保持在本计算机上。",
                [MALFORMED]
            ),
            Err(Mismatch::InNotesRelation)
        );
    }

    #[test]
    fn a_fixed_in_clause_does_not_hide_an_extra_and_relation() {
        for extra in [
            "录音和会议笔记应保留在本机。",
            "录音以及会议笔记应保留在本机。",
            "录音和会议笔记是两个项目。",
            "Recordings and meeting notes should stay on this computer.",
        ] {
            let body = format!("会议笔记中的录音应保留在本机。{extra}");
            assert_eq!(validate(&body, [MALFORMED]), Err(Mismatch::InNotesRelation));
        }
    }

    #[test]
    fn source_anchor_consumes_only_the_complete_retention_frame() {
        let fixed = anchor(MALFORMED, true).unwrap();
        assert_eq!(fixed.text, "会议笔记中的录音应保留在这台电脑上。");
        assert_eq!(fixed.required_object_phrase, "会议笔记中的录音");
        assert_eq!(
            &MALFORMED[fixed.source_span.clone()],
            "RECORDING IN MEETING NOTES SHOULD STAY ON THIS COMPUTER"
        );
        assert_eq!(fixed.remaining_source, "FRIDAY IS NOT THE DEADLINE ");
        assert!(fixed.has_other_context);
        assert!(validate(&fixed.text, [MALFORMED]).is_ok());

        let english = anchor(MALFORMED, false).unwrap();
        assert_eq!(
            english.text,
            "The recording in meeting notes should remain on this computer."
        );
        assert!(validate(&english.text, [MALFORMED]).is_ok());
    }

    #[test]
    fn standalone_anchor_preserves_modality_destination_and_coordination() {
        for (source, english, chinese) in [
            (
                "Audio recordings and meeting notes must be kept on this device.",
                "The audio recordings and meeting notes must remain on this device.",
                "录音和会议笔记必须保留在此设备上。",
            ),
            (
                "The recording should stay on this computer.",
                "The recording should remain on this computer.",
                "录音应保留在这台电脑上。",
            ),
            (
                "Keep the recording locally.",
                "Keep the recording locally.",
                "请将录音保留在本地。",
            ),
            (
                "会议笔记中的录音应保持在本计算机上。",
                "The recording in meeting notes should remain on this computer.",
                "会议笔记中的录音应保留在这台电脑上。",
            ),
        ] {
            for (target, expected) in [(false, english), (true, chinese)] {
                let a = anchor(source, target).unwrap();
                assert_eq!(a.text, expected);
                assert_eq!(a.source_span, 0..source.len());
                assert!(!a.has_other_context);
                assert!(a.remaining_source.is_empty());
                assert!(validate(&a.text, [source]).is_ok());
            }
        }
    }

    #[test]
    fn later_source_context_remains_exact_original_text() {
        let source = "The recording should stay on this computer. The budget is not approved.";
        let a = anchor(source, true).unwrap();
        assert!(a.has_other_context);
        assert_eq!(a.remaining_source, ". The budget is not approved.");
        assert_eq!(
            format!(
                "{}{}",
                &source[..a.source_span.start],
                &source[a.source_span.end..]
            ),
            a.remaining_source
        );
        assert!(!source[a.source_span].contains("budget"));
    }

    #[test]
    fn anchors_abstain_on_unknown_modifiers_names_conditions_and_other_destinations() {
        for source in [
            "The confidential recording should stay on this computer.",
            "The original recording should stay on this computer.",
            "The recording should stay only on this computer.",
            "The recording should stay on this computer temporarily.",
            "The recording should stay on this computer and an external drive.",
            "The recording should stay on this computer. It must also be copied to an external drive.",
            "If approved, the recording should stay on this computer.",
            "If approved. The recording should stay on this computer.",
            "The recording should not stay on this computer.",
            "The recording should stay on this computer instead of cloud storage.",
            "The recording should stay on this computer. It was moved to cloud storage.",
            "Project Recording should stay on this computer.",
            "\"Recording\" should stay on this computer.",
            "The recording was saved on this computer.",
            "Recording decisions in meeting notes should stay on this computer.",
            "The recording should stay on this computer?",
            "The recording should stay on this computer. The recording must stay on this device.",
            "会议笔记的录音应保持在本计算机上。",
            "机密录音应保存在本机。",
        ] {
            assert!(anchor(source, true).is_none(), "{source}");
            assert!(anchor(source, false).is_none(), "{source}");
        }
    }
}
