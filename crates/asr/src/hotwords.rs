//! Export the pinned SentencePiece vocabulary locally; no tokenizer service or network needed.
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
fn varint(data: &[u8], cursor: &mut usize) -> Result<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(*cursor).context("Truncated SentencePiece model")?;
        *cursor += 1;
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    bail!("Invalid SentencePiece varint")
}
fn fields(data: &[u8]) -> Result<Vec<(u64, &[u8])>> {
    let mut cursor = 0;
    let mut result = Vec::new();
    while cursor < data.len() {
        let tag = varint(data, &mut cursor)?;
        let size = match tag & 7 {
            0 => {
                varint(data, &mut cursor)?;
                continue;
            }
            1 => 8,
            2 => usize::try_from(varint(data, &mut cursor)?)?,
            5 => 4,
            _ => bail!("Invalid SentencePiece field"),
        };
        let end = cursor.checked_add(size).context("Invalid model size")?;
        let bytes = data
            .get(cursor..end)
            .context("Truncated SentencePiece field")?;
        if tag & 7 == 2 || tag & 7 == 5 {
            result.push((tag >> 3, bytes));
        }
        cursor = end;
    }
    Ok(result)
}
pub fn vocabulary(model: &Path, audio: &Path, shared: Option<&Path>) -> Result<PathBuf> {
    let tokenizer = if model.join("bpe.model").is_file() {
        model.join("bpe.model")
    } else {
        let shared = shared.context("No local hotword tokenizer")?;
        // Sharing is safe only for precisely identical acoustic token inventories.
        let tokens = std::fs::read(model.join("tokens.txt"))?;
        if tokens.is_empty()
            || tokens.len() > 1_000_000
            || tokens != std::fs::read(shared.join("tokens.txt"))?
        {
            bail!("Incompatible shared hotword tokenizer");
        }
        shared.join("bpe.model")
    };
    let data = std::fs::read(tokenizer)?;
    if data.len() > 2_000_000 {
        bail!("Tokenizer model too large");
    }
    let mut pieces = Vec::new();
    for (_, piece) in fields(&data)?.into_iter().filter(|(field, _)| *field == 1) {
        let piece_fields = fields(piece)?;
        let score = piece_fields
            .iter()
            .find(|(field, _)| *field == 2)
            .filter(|(_, v)| v.len() == 4)
            .map(|(_, v)| f32::from_le_bytes((*v).try_into().unwrap()))
            .unwrap_or(0.);
        let piece = piece_fields
            .into_iter()
            .find(|(field, _)| *field == 1)
            .context("Missing SentencePiece token")?
            .1;
        let text = std::str::from_utf8(piece)?;
        if text.contains(['\n', '\r', '\t']) {
            bail!("Invalid vocabulary token");
        }
        pieces.push(format!("{text}\t{score}"));
    }
    if pieces.is_empty() {
        bail!("Empty tokenizer vocabulary");
    }
    let path = audio.join("bpe.vocab");
    std::fs::write(&path, pieces.join("\n"))?;
    Ok(path)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_truncated_model_and_reads_length_fields() {
        assert!(fields(&[10, 9, 1]).is_err());
        assert_eq!(fields(&[10, 2, b'A', b'B']).unwrap(), vec![(1, &b"AB"[..])]);
    }
}
