//! Bounded translation units and private, revision-bound resumable work.
use crate::{store::Store, Segment};
use anyhow::{bail, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

pub(crate) const INPUT_TOKENS: usize = 256;
pub(crate) const OUTPUT_TOKENS: u32 = 768;
pub(super) const MAX_REQUESTS: usize = 8;
const MAX_INPUT_BYTES: usize = 640;
const MAX_OUTPUT_CHARS: usize = 16_000;

#[derive(Debug)]
pub(super) struct DoesNotFit;
impl std::fmt::Display for DoesNotFit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Source cannot fit safely within the translation token budget")
    }
}
impl std::error::Error for DoesNotFit {}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Progress {
    pub end: usize,
    pub text: String,
}
impl Progress {
    pub fn load(db: &Store, source: &Segment, key: &str) -> Result<Self> {
        let row: Option<String> = db
            .conn
            .query_row(
                "SELECT json FROM translation_parts WHERE segment_id=?1 AND cache_key=?2",
                rusqlite::params![source.id, key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row
            .and_then(|row| serde_json::from_str::<Self>(&row).ok())
            .filter(|p| {
                p.end > 0
                    && p.end < source.text.len()
                    && source.text.is_char_boundary(p.end)
                    && !source.text[p.end..].trim().is_empty()
                    && !p.text.trim().is_empty()
                    && p.text.chars().count() <= MAX_OUTPUT_CHARS
            })
            .unwrap_or_default())
    }
    pub fn append(&mut self, source: &str, end: usize, text: &str, target: &str) -> Result<()> {
        if end <= self.end
            || end > source.len()
            || !source.is_char_boundary(end)
            || text.trim().is_empty()
        {
            bail!("Invalid translation progress");
        }
        if !self.text.is_empty() {
            if source[self.end..end]
                .trim_start_matches([' ', '\t'])
                .starts_with(['\n', '\r'])
            {
                self.text.push('\n');
            } else if target == "en" {
                self.text.push(' ');
            }
        }
        self.text.push_str(text.trim());
        if self.text.chars().count() > MAX_OUTPUT_CHARS {
            bail!("Translation exceeds output budget");
        }
        self.end = if source[end..].trim().is_empty() {
            source.len()
        } else {
            end
        };
        Ok(())
    }
}

pub(super) struct Chunk {
    pub start: usize,
    pub end: usize,
    pub tokens: usize,
}
fn boundary(text: &str, limit: usize) -> usize {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end == text.len() {
        return end;
    }
    let prefix = &text[..end];
    // Prefer a complete sentence, then a word. Never cut an ASCII name/number in half.
    for punctuation in [true, false] {
        if let Some((i, c)) = prefix.char_indices().rev().find(|(i, c)| {
            *i >= end / 2
                && if punctuation {
                    "。！？!?；;\n".contains(*c)
                        || *c == '.' && text[i + 1..].chars().next().is_none_or(char::is_whitespace)
                } else {
                    c.is_whitespace()
                }
        }) {
            return i + c.len_utf8();
        }
    }
    let protected = |c: char| c.is_ascii_alphanumeric() || "._+-:/@".contains(c);
    while end > 0
        && text[..end].chars().next_back().is_some_and(protected)
        && text[end..].chars().next().is_some_and(protected)
    {
        end -= text[..end].chars().next_back().unwrap().len_utf8();
    }
    end
}
/// The callback counts the actual runtime tokens, including the system instruction.
pub(super) fn next(
    source: &str,
    offset: usize,
    budget: usize,
    mut count: impl FnMut(&str) -> Result<usize>,
) -> Result<Option<Chunk>> {
    if !source.is_char_boundary(offset) {
        bail!("Invalid source cursor");
    }
    let remaining = &source[offset..];
    let start = offset + remaining.len() - remaining.trim_start().len();
    let remaining = &source[start..];
    if remaining.is_empty() {
        return Ok(None);
    }
    let mut end = boundary(remaining, MAX_INPUT_BYTES);
    loop {
        if end == 0 {
            return Err(DoesNotFit.into());
        }
        let tokens = count(&remaining[..end])?;
        if tokens <= budget {
            return Ok(Some(Chunk {
                start,
                end: start + end,
                tokens,
            }));
        }
        let smaller = boundary(remaining, end / 2);
        end = if smaller < end { smaller } else { 0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_limited_chunks_preserve_unicode_and_all_source_content() -> Result<()> {
        let source =
            "预算尚未批准；周五再讨论。\nMorgan will send the report on Thursday. ".repeat(90);
        let (mut cursor, mut rebuilt, mut count) = (0, String::new(), 0);
        while let Some(chunk) = next(&source, cursor, 75, |s| Ok(s.chars().count() + 20))? {
            assert!(chunk.tokens <= 75 && chunk.end > cursor);
            rebuilt.push_str(&source[cursor..chunk.end]);
            cursor = chunk.end;
            count += 1;
        }
        rebuilt.push_str(&source[cursor..]);
        assert_eq!(source, rebuilt);
        assert!(count > 8);
        for source in [
            format!("{}3000.50元", "中".repeat(212)),
            format!("{}PixelLedger测试", "中".repeat(212)),
        ] {
            let chunk = next(&source, 0, 1000, |_| Ok(1))?.unwrap();
            assert!(
                !source[chunk.end..].starts_with("0.50")
                    && !source[chunk.end..].starts_with("Ledger")
            );
            assert!(source[..chunk.end].ends_with('中'));
        }
        assert!(next("中", 0, 1, |_| Ok(2)).is_err());
        assert!(next("中", 1, 75, |_| Ok(1)).is_err());
        assert!(next("   \n", 0, 75, |_| panic!(
            "Whitespace must not start a request"
        ))
        .unwrap()
        .is_none());
        Ok(())
    }
    #[test]
    fn private_progress_requires_exact_key_and_current_source_boundary() -> Result<()> {
        let root = tempfile::tempdir()?;
        let db = Store::open(&root.path().join("cache.sqlite3"))?;
        let source = Segment {
            id: "s".into(),
            meeting_id: "m".into(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "中文。下一句".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        db.save_segment(&source)?;
        let progress = Progress {
            end: 9,
            text: "Chinese.".into(),
        };
        db.conn.execute(
            "INSERT INTO translation_parts VALUES(?1,?2,?3)",
            rusqlite::params![source.id, "current", serde_json::to_string(&progress)?],
        )?;
        assert_eq!(Progress::load(&db, &source, "current")?.end, 9);
        assert_eq!(Progress::load(&db, &source, "different-config")?.end, 0);
        db.conn.execute(
            "UPDATE translation_parts SET json=?1",
            [r#"{"end":1,"text":"bad cursor"}"#],
        )?;
        assert_eq!(Progress::load(&db, &source, "current")?.end, 0);
        db.conn.execute(
            "UPDATE translation_parts SET json=?1",
            [serde_json::to_string(&progress)?],
        )?;
        let mut edited = source.clone();
        edited.revision += 1;
        edited.text = "人工文字".into();
        db.save_segment(&edited)?;
        assert_eq!(Progress::load(&db, &edited, "current")?.end, 0);
        Ok(())
    }
}
