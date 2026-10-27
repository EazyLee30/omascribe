//! Explicit phase scope and UI placement intent, not general entailment.
use anyhow::Result;
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Mismatch {
    Scope,
    Placement,
}
impl Mismatch {
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Scope => " Preserve the stated implementation scope: only/limited to qualifies this phase or version, not an unrelated time. Do not add a restriction absent from the speech.",
            Self::Placement => " Preserve whether toolbar placement is requested or already done. Use should or an imperative for a request; an existing entry is a fact, not a new request.",
        }
    }
}
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.hint().trim())
    }
}
impl std::error::Error for Mismatch {}
struct Scope {
    version: bool,
    verb: &'static str,
    exclusive: bool,
    future: bool,
}
fn scope(source: &str) -> Option<Scope> {
    static ZH: OnceLock<Regex> = OnceLock::new();
    static EN: OnceLock<Regex> = OnceLock::new();
    let frame=ZH.get_or_init(||Regex::new(r"(?:^|[。.!?！？；;\n])\s*(?P<subject>本阶段|这个阶段|当前阶段|当前版本|本版本)(?P<future>将|会)?(?P<only>只|仅)?(?P<verb>实现|支持|包含)").unwrap()).captures(source)
        .or_else(||EN.get_or_init(||Regex::new(r"(?i)(?:^|[.!?;\n])\s*(?P<subject>this phase|this stage|the current version)\s+(?P<future>will\s+)?(?P<only>only\s+)?(?P<verb>implements?|supports?|includes?)(?P<post>\s+only\b)?").unwrap()).captures(source))?;
    let clause = source[frame.name("subject")?.start()..]
        .split(['。', '.', ';', '；', '\n'])
        .next()?;
    if clause.contains(['?', '？']) {
        return None;
    }
    let operand = source[frame.get(0)?.end()..]
        .split(['，', ',', '。', '.', ';', '；', '\n'])
        .next()
        .unwrap_or("");
    if !operand.chars().any(char::is_alphanumeric) {
        return None;
    }
    if frame.name("post").is_some()
        && ["on ", "at ", "before ", "after ", "until ", "when "]
            .iter()
            .any(|p| {
                source[frame.get(0).unwrap().end()..]
                    .trim_start()
                    .to_ascii_lowercase()
                    .starts_with(p)
            })
    {
        return None;
    }
    let verb = frame["verb"].to_ascii_lowercase();
    Some(Scope {
        version: frame["subject"].contains("版本")
            || frame["subject"].to_ascii_lowercase().contains("version"),
        verb: if verb == "支持" || verb.starts_with("support") {
            "支持"
        } else if verb == "包含" || verb.starts_with("include") {
            "包含"
        } else {
            "实现"
        },
        exclusive: frame.name("only").is_some() || frame.name("post").is_some(),
        future: frame.name("future").is_some(),
    })
}
fn withdrawn(source: &str) -> bool {
    let s = source.to_ascii_lowercase();
    [
        "取消",
        "撤回",
        "不再要求",
        "已经否决",
        "cancelled",
        "canceled",
        "withdrawn",
        "no longer required",
    ]
    .iter()
    .any(|v| s.contains(v))
}
struct Placement {
    requested: bool,
    start: usize,
}
fn placement(source: &str) -> Option<Placement> {
    if withdrawn(source) {
        return None;
    }
    static REQUEST: OnceLock<Regex> = OnceLock::new();
    static EXISTING: OnceLock<Regex> = OnceLock::new();
    let request=REQUEST.get_or_init(||Regex::new(r"(?:^|[，,。；;\n])\s*(?:请|要|应|应该|需要)?(?:把)?入口放在工具栏[，,；;]\s*(?:别|不要|勿)(?:藏|隐藏)").unwrap()).find(source);
    let existing = EXISTING
        .get_or_init(|| Regex::new(r"入口(?:已经|已)(?:放在|放到|位于)工具栏").unwrap())
        .find(source);
    match (request, existing) {
        (Some(m), None) => Some(Placement {
            requested: true,
            start: m.start(),
        }),
        (None, Some(m)) => Some(Placement {
            requested: false,
            start: m.start(),
        }),
        _ => None,
    }
}
#[derive(Clone, Copy)]
enum Feature {
    Transcript,
    Captions,
}
impl Feature {
    fn label(self) -> &'static str {
        match self {
            Self::Transcript => "real-time transcription",
            Self::Captions => "live captions",
        }
    }
    fn mentioned(self, text: &str) -> bool {
        static TRANSCRIPT: OnceLock<Regex> = OnceLock::new();
        static CAPTIONS: OnceLock<Regex> = OnceLock::new();
        match self {
            Self::Transcript => TRANSCRIPT.get_or_init(|| {
                Regex::new(r"(?i)实时转写|\b(?:live|real[- ]?time)\s+transcri(?:ption|pt)s?\b")
                    .unwrap()
            }),
            Self::Captions => CAPTIONS.get_or_init(|| {
                Regex::new(r"(?i)实时字幕|\b(?:live|real[- ]?time)\s+(?:captions?|subtitles?)\b")
                    .unwrap()
            }),
        }
        .is_match(text)
    }
}
fn features(source: &str, placement: &Placement) -> Vec<Feature> {
    // Bind the placement to its immediately preceding topic, not to features
    // mentioned as unrelated facts earlier in the meeting.
    let topic = source[..placement.start]
        .rsplit(['，', ',', '。', '.', '；', ';', '\n'])
        .next()
        .unwrap_or("");
    [Feature::Transcript, Feature::Captions]
        .into_iter()
        .filter(|f| f.mentioned(topic))
        .collect()
}
fn available(clause: &str) -> bool {
    static AVAILABLE: OnceLock<Regex> = OnceLock::new();
    AVAILABLE.get_or_init(||Regex::new(r"(?i)\b(?:is|are)\s+(?:already\s+)?(?:available|ready|enabled)\b|(?:已经|已)(?:能显示|可用|启用|上线|就绪)").unwrap()).is_match(clause)
}
fn requested(clause: &str) -> bool {
    static MODAL: OnceLock<Regex> = OnceLock::new();
    MODAL.get_or_init(||Regex::new(r"(?i)\b(?:should|must|needs?\s+to|please|requested)\b|^\s*(?:place|put|keep)\b|应当|应该|需要|请|别藏|不要藏|勿藏").unwrap()).is_match(clause)
}
pub(crate) fn prefix(source: &str, chinese: bool) -> Option<String> {
    if withdrawn(source) {
        return None;
    }
    if let Some(s) = scope(source).filter(|s| s.exclusive) {
        if chinese {
            return Some(format!(
                "{}{}只{}",
                if s.version {
                    "当前版本"
                } else {
                    "本阶段"
                },
                if s.future { "将" } else { "" },
                s.verb
            ));
        }
        let subject = if s.version {
            "The current version"
        } else {
            "This phase"
        };
        let (present, base) = match s.verb {
            "支持" => ("supports", "support"),
            "包含" => ("includes", "include"),
            _ => ("implements", "implement"),
        };
        return Some(if s.future {
            format!("{subject} will only {base} ")
        } else {
            format!("{subject} only {present} ")
        });
    }
    if !chinese {
        return placement(source).map(|p| {
            let subjects = features(source, &p)
                .into_iter()
                .map(Feature::label)
                .collect::<Vec<_>>()
                .join(" and ");
            let subject = if subjects.is_empty() {
                "The entry points".into()
            } else {
                format!("The entry points for {subjects}")
            };
            if p.requested {
                format!("{subject} should be placed ")
            } else {
                format!("{subject} are already in the toolbar ")
            }
        });
    }
    None
}
/// Keep independently qualified source sentences in separate bounded chunks.
/// Offsets remain in the original UTF-8 text; no speech is rewritten or lost.
pub(crate) fn boundary(source: &str, chinese: bool) -> Option<usize> {
    let mut seen = false;
    let mut offset = 0;
    for sentence in source.split_inclusive(['。', '\n']) {
        if prefix(sentence, chinese).is_some() {
            if seen {
                return Some(offset);
            }
            seen = true;
        }
        offset += sentence.len();
    }
    None
}
pub(super) fn validate(body: &str, source: &str) -> Result<()> {
    if withdrawn(source) {
        return Ok(());
    }
    let lower = body.to_lowercase();
    let clauses: Vec<_> = lower.split(['。', '.', ';', '；', '\n']).collect();
    if let Some(s) = scope(source) {
        let subjects = if s.version {
            &["版本", "version"][..]
        } else {
            &["阶段", "本轮", "phase", "stage"][..]
        };
        let matching: Vec<_> = clauses
            .iter()
            .filter(|c| {
                subjects.iter().any(|p| c.contains(p))
                    && [
                        "实现",
                        "支持",
                        "包含",
                        "范围",
                        "implement",
                        "support",
                        "include",
                        "cover",
                        "limited",
                        "restricted",
                    ]
                    .iter()
                    .any(|v| c.contains(v))
            })
            .collect();
        static RESTRICT: OnceLock<Regex> = OnceLock::new();
        static TIME_ONLY: OnceLock<Regex> = OnceLock::new();
        let exclusive = matching.iter().any(|c| {
            RESTRICT
                .get_or_init(|| {
                    Regex::new(r"(?i)\bonly\b|\bsolely\b|limited\s+to|restricted\s+to|只|仅|限于")
                        .unwrap()
                })
                .is_match(c)
                && !c.contains("not only")
                && !c.contains("不仅")
                && !TIME_ONLY
                    .get_or_init(|| {
                        Regex::new(
                            r"(?i)\bonly\s+(?:on|at|before|after|until|when|today|tomorrow)\b",
                        )
                        .unwrap()
                    })
                    .is_match(c)
        });
        if (!matching.is_empty() && exclusive != s.exclusive)
            || (matching.is_empty() && s.exclusive)
        {
            return Err(Mismatch::Scope.into());
        }
        if s.future
            && !matching.iter().any(|c| {
                [
                    "will ", "planned", "plan to", "intend", "将", "计划", "拟", "会",
                ]
                .iter()
                .any(|v| c.contains(v))
            })
        {
            return Err(Mismatch::Scope.into());
        }
    }
    if let Some(placement) = placement(source) {
        let topics = features(source, &placement);
        if topics.iter().any(|f| !f.mentioned(body)) {
            return Err(Mismatch::Placement.into());
        }
        let matching: Vec<_> = clauses
            .iter()
            .filter(|c| c.contains("toolbar") || c.contains("工具栏"))
            .collect();
        if matching.is_empty() || matching.iter().any(|c| requested(c)) != placement.requested {
            return Err(Mismatch::Placement.into());
        }
        if placement.requested {
            for feature in topics {
                let supported = source
                    .split(['。', '.', '，', ',', ';', '；', '\n'])
                    .any(|c| feature.mentioned(c) && available(c));
                if !supported
                    && clauses.iter().any(|c| {
                        available(c)
                            && (feature.mentioned(c)
                                || ["these features", "both features", "they are"]
                                    .iter()
                                    .any(|p| c.contains(p)))
                    })
                {
                    return Err(Mismatch::Placement.into());
                }
            }
        }
        static OPPOSITE: OnceLock<Regex> = OnceLock::new();
        if matching.iter().any(|c|OPPOSITE.get_or_init(||Regex::new(r"(?i)\b(?:should|must)\s+not\s+(?:be\s+)?(?:placed|put|located|in)\b[^.;]{0,20}\btoolbar\b|\b(?:are|is)\s+not\s+(?:already\s+)?(?:placed\s+)?in\s+(?:the\s+)?toolbar\b|入口(?:不应|不应该|不可|不能)[^。；]{0,8}放(?:在|到)工具栏|不要把入口放在工具栏").unwrap()).is_match(c)) {
            return Err(Mismatch::Placement.into());
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn placement_preserves_the_named_features_without_inventing_availability() {
        let source = "还有实时转写和实时字幕，入口放在工具栏，别藏到二级菜单。";
        for body in [
            "The entry points should be placed in the toolbar, not hidden in a secondary menu.",
            "The entry points should be placed in the toolbar, not hidden in a secondary menu. Real-time transcription and real-time subtitles are available.",
        ] {
            assert!(validate(body, source).is_err(), "{body}");
        }
        assert!(validate("The entry points for real-time transcription and live captions should be placed in the toolbar, not hidden in a secondary menu.", source).is_ok());
    }
    #[test]
    fn scope_is_bound_to_implementation_not_an_unrelated_day() {
        let source = "本阶段只实现麦克风录音与实时转写。";
        for body in [
            "本阶段仅实现麦克风录音与实时转写。",
            "This phase only implements microphone recording and real-time transcription.",
            "This phase is limited to microphone recording and real-time transcription.",
        ] {
            assert!(validate(body, source).is_ok(), "{body}");
        }
        for body in [
            "This phase implements microphone recording.",
            "This phase implements microphone recording and is tested only on Friday.",
            "This phase not only implements microphone recording but also system audio.",
        ] {
            assert!(validate(body, source).is_err(), "{body}");
        }
        assert!(validate(
            "This phase only supports microphone recording.",
            "本阶段支持麦克风录音。"
        )
        .is_err());
        assert_eq!(
            prefix("This phase will only support microphone recording.", true).as_deref(),
            Some("本阶段将只支持")
        );
        assert!(validate(
            "本阶段只支持麦克风录音。",
            "This phase will only support microphone recording."
        )
        .is_err());
        assert!(prefix("本阶段只实现麦克风录音吗？", false).is_none());
        assert!(prefix("已有讨论。\n本阶段只实现麦克风录音吗？", false).is_none());
        assert!(prefix("This phase supports only on Friday.", true).is_none());
        assert!(prefix("本阶段只实现……", false).is_none());
    }
    #[test]
    fn request_and_already_placed_entries_remain_distinct() {
        let request = "还有实时转写和实时字幕，入口放在工具栏，别藏到二级菜单。";
        let existing = "实时转写和实时字幕的入口已经放在工具栏，没有藏在二级菜单。";
        for body in [
            "The entry points for real-time transcription and live captions should be placed in the toolbar, not hidden in secondary menus.",
            "Place the live transcript and real-time subtitles entry points in the toolbar rather than in a secondary menu.",
            request,
        ] {
            assert!(validate(body, request).is_ok(), "{body}");
        }
        assert!(validate("The entry points are already in the toolbar.", request).is_err());
        assert!(validate(
            "The entry points should not be placed in the toolbar.",
            request
        )
        .is_err());
        assert!(validate("入口不应该放在工具栏。", request).is_err());
        assert!(validate(
            "The entry points should be placed in the toolbar.",
            existing
        )
        .is_err());
        assert!(validate(
            "The entry points for live transcription and live captions are already in the toolbar.",
            existing
        )
        .is_ok());
        assert!(prefix("不要把入口放在工具栏，别藏到二级菜单。", false).is_none());
        assert!(prefix("入口放在工具栏，别藏到二级菜单。这个要求已经取消。", false).is_none());
    }
    #[test]
    fn topic_and_availability_are_local_to_the_feature() {
        let source = "实时转写已经能显示。实时字幕，入口放在工具栏，别藏到二级菜单。";
        let prefix = prefix(source, false).unwrap();
        assert_eq!(
            prefix,
            "The entry points for live captions should be placed "
        );
        assert!(validate("Real-time transcription is available. Live captions should be placed in the toolbar, not hidden in a secondary menu.", source).is_ok());
        assert!(validate("Real-time transcription and live captions are available. Live captions should be placed in the toolbar.", source).is_err());
        assert!(validate("Live captions are already available; their entry points should be placed in the toolbar.", "实时字幕已可用。实时字幕，入口放在工具栏，别藏到二级菜单。").is_ok());
    }
    #[test]
    fn independent_qualifiers_have_original_utf8_sentence_boundaries() {
        let first = "本阶段只实现麦克风录音与实时转写。";
        let second = "实时转写和实时字幕，入口放在工具栏，别藏到二级菜单。这两项调整还没有完成。";
        assert_eq!(
            boundary(&format!("{first}{second}"), false),
            Some(first.len())
        );
        assert_eq!(boundary(first, false), None);
        assert_eq!(boundary(second, false), None);
        assert_eq!(
            boundary("本阶段只实现麦克风录音，系统音频推迟到下个版本。", false),
            None
        );
    }
}
