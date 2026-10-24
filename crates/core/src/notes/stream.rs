//! OpenAI-compatible SSE, committed only after a complete, successful response.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::{BufRead, Read};

pub fn read(mut reader: impl BufRead, mut progress: impl FnMut(usize)) -> Result<(String, Value)> {
    let mut text = String::new();
    let mut usage = Value::Null;
    let mut stopped = false;
    loop {
        let mut line = String::new();
        // Limit each line and the accumulated completion, including malformed responses.
        let n = reader.by_ref().take(65_537).read_line(&mut line)?;
        if n == 0 {
            bail!("生成连接提前结束，已有文档未改动，稍后自动重试");
        }
        if n > 65_536 {
            bail!("模型返回的数据过长");
        }
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            if !stopped || text.trim().is_empty() {
                bail!("纪要输出不完整，本轮保留待处理内容");
            }
            return Ok((text, usage));
        }
        let event: Value = serde_json::from_str(data).context("模型流格式无效")?;
        if event.get("error").is_some() {
            bail!("本地模型报告生成失败，稍后自动重试");
        }
        if !event["usage"].is_null() {
            usage = event["usage"].clone();
        }
        if let Some(part) = event["choices"][0]["delta"]["content"].as_str() {
            text.push_str(part);
            if text.len() > 262_144 {
                bail!("模型返回的数据过长");
            }
            progress(text.chars().count());
        }
        if let Some(reason) = event["choices"][0]["finish_reason"].as_str() {
            if reason != "stop" {
                bail!("纪要输出不完整，本轮保留待处理内容");
            }
            stopped = true;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_unicode_stream_and_usage() -> Result<()> {
        let stream = concat!(": keepalive\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"预算\"}}]}\r\n\r\n", "data: {\"choices\":[{\"delta\":{\"content\":\"未定\"},\"finish_reason\":\"stop\"}]}\n\n", "data: {\"choices\":[],\"usage\":{\"completion_tokens\":4}}\n\ndata: [DONE]\n\n");
        let mut counts = Vec::new();
        let (text, usage) = read(stream.as_bytes(), |n| counts.push(n))?;
        assert_eq!(text, "预算未定");
        assert_eq!(counts, vec![2, 4]);
        assert_eq!(usage["completion_tokens"], 4);
        Ok(())
    }
    #[test]
    fn truncation_error_and_unbounded_data_cannot_commit() {
        for stream in [
            "data: [DONE]\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n",
            "data: {\"choices\":[{\"finish_reason\":\"length\"}]}\n",
            "data: {\"error\":{}}\n",
        ] {
            assert!(read(stream.as_bytes(), |_| {}).is_err());
        }
        assert!(read(vec![b'x'; 70_000].as_slice(), |_| {}).is_err());
    }
}
