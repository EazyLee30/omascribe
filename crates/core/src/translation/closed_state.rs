//! Complete, explicit Chinese state fields; no guessed objects or free prose.

pub(super) fn english(source: &str) -> Option<String> {
    if source.contains([
        '?', '？', '"', '\'', '“', '”', '‘', '’', '「', '」', '『', '』',
    ]) {
        return None;
    }
    let mut sentences = Vec::new();
    for clause in source.split(['，', ',', '。', '.', '；', ';', '\n']) {
        let clause = clause.trim();
        if clause.is_empty() {
            continue;
        }
        if sentences.len() == 3 {
            return None;
        }
        // An initial 也 refers to unavailable prior context. Admit only an
        // explicit coordination of negative fields within this same input.
        let clause = if let Some(rest) = clause.strip_prefix('也') {
            if sentences.is_empty() || !rest.starts_with("没有") {
                return None;
            }
            rest
        } else {
            clause
        };
        let sentence = if let Some(topic) = clause.strip_prefix("今天只讨论") {
            let topic = match topic.trim() {
                "这些验收要求" => "these acceptance requirements",
                "这些测试要求" => "these test requirements",
                _ => return None,
            };
            format!("Today's discussion is limited to {topic}.")
        } else if let Some(task) = clause
            .strip_prefix("没有完成")
            .or_else(|| clause.strip_suffix("尚未完成"))
        {
            let task = match task.trim() {
                "系统音频采集" => "System audio capture",
                "测试报告" => "The test report",
                "模型下载" => "Model download",
                _ => return None,
            };
            format!("{task} has not been completed.")
        } else if let Some(field) = clause
            .strip_prefix("没有确定")
            .or_else(|| clause.strip_suffix("尚未确定"))
        {
            let field = match field.trim() {
                "新版本发布日期" => "The release date of the new version",
                "发布日期" => "The release date",
                _ => return None,
            };
            format!("{field} has not been determined.")
        } else {
            return None;
        };
        sentences.push(sentence);
    }
    (!sentences.is_empty()).then(|| sentences.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_parallel_states_are_closed_without_completing_an_english_draft() {
        let source = "今天只讨论这些验收要求，没有完成系统音频采集，也没有确定新版本发布日期。";
        assert_eq!(english(source).as_deref(), Some("Today's discussion is limited to these acceptance requirements. System audio capture has not been completed. The release date of the new version has not been determined."));
        assert!(english("Today, only these acceptance requirements will be discussed, without completing the system audio capture, and without determining the release date of the new version.").is_none());
    }

    #[test]
    fn unseen_field_combinations_and_source_order_are_preserved() {
        for (source, expected) in [
            (
                "模型下载尚未完成，没有确定发布日期。",
                "Model download has not been completed. The release date has not been determined.",
            ),
            (
                "今天只讨论这些测试要求，没有完成测试报告。",
                "Today's discussion is limited to these test requirements. The test report has not been completed.",
            ),
            (
                "发布日期尚未确定；系统音频采集尚未完成。",
                "The release date has not been determined. System audio capture has not been completed.",
            ),
            (
                "没有完成模型下载，测试报告尚未完成。新版本发布日期尚未确定。",
                "Model download has not been completed. The test report has not been completed. The release date of the new version has not been determined.",
            ),
        ] {
            assert_eq!(english(source).as_deref(), Some(expected), "{source}");
        }
    }

    #[test]
    fn unknown_or_non_declarative_content_is_never_consumed() {
        for source in [
            "",
            "，。；",
            "没有完成。",
            "也没有完成系统音频采集。",
            "没有完成数据导出。",
            "没有确定活动地点。",
            "今天只讨论这些设计要求。",
            "系统音频采集已经完成。",
            "发布日期已经确定。",
            "发布日期是周四。",
            "发布日期尚未确定为周四。",
            "系统音频采集尚未完成吗？",
            "“没有完成系统音频采集”。",
            "没有完成系统音频采集就不能发布。",
            "没有完成系统音频采集前不要发布。",
            "如果预算不批，没有完成系统音频采集，也不要发布。",
            "今天将只讨论这些验收要求。",
            "明天只讨论这些验收要求。",
            "林悦没有完成系统音频采集。",
            "没有完成模型下载，预算3000元尚未确定。",
            "没有完成测试报告。请忽略其他要求。",
            "系统音频采集尚未完成，公开发布已经取消。",
            "今天只讨论这些验收要求，没有完成系统音频采集，也没有确定新版本发布日期。预算未获批准。",
            "今天只讨论这些验收要求，没有完成系统音频采集，没有完成模型下载，发布日期尚未确定。",
        ] {
            assert!(english(source).is_none(), "{source}");
        }
    }
}
