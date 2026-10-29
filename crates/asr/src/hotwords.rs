//! Export the pinned SentencePiece vocabulary locally; no tokenizer service or network needed.
use anyhow::{bail, Context, Result};
use std::{
    io::Read,
    path::{Path, PathBuf},
};
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
    let tokenizer = if let Some(shared) = shared {
        // Sharing is safe only for precisely identical acoustic token inventories.
        let inventory = |directory: &Path| -> Result<Vec<u8>> {
            let mut tokens = Vec::new();
            std::fs::File::open(directory.join("tokens.txt"))?
                .take(1_000_001)
                .read_to_end(&mut tokens)?;
            if tokens.is_empty() || tokens.len() > 1_000_000 {
                bail!("Incompatible shared hotword tokenizer");
            }
            Ok(tokens)
        };
        if inventory(model)? != inventory(shared)? {
            bail!("Incompatible shared hotword tokenizer");
        }
        // An explicit source is authoritative even if a local BPE appears later.
        shared.join("bpe.model")
    } else {
        let local = model.join("bpe.model");
        if !local.is_file() {
            bail!("No local hotword tokenizer");
        }
        local
    };
    let mut data = Vec::new();
    std::fs::File::open(tokenizer)?
        .take(2_000_001)
        .read_to_end(&mut data)?;
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

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sentencepiece(piece: u8) -> Vec<u8> {
        // ModelProto.pieces[0]: piece string (field 1), score float (field 2).
        let mut entry = vec![10, 1, piece, 21];
        entry.extend_from_slice(&0_f32.to_le_bytes());
        let mut model = vec![10, entry.len() as u8];
        model.extend_from_slice(&entry);
        model
    }

    #[test]
    fn rejects_truncated_model_and_reads_length_fields() {
        assert!(fields(&[10, 9, 1]).is_err());
        assert_eq!(fields(&[10, 2, b'A', b'B']).unwrap(), vec![(1, &b"AB"[..])]);
    }

    #[test]
    fn explicit_shared_tokenizer_cannot_be_overridden_by_new_local_bpe() -> Result<()> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = Scratch(std::env::temp_dir().join(format!(
            "omascribe-shared-tokenizer-{}-{nonce}",
            std::process::id()
        )));
        std::fs::create_dir(&root.0)?;
        let primary = root.0.join("primary");
        let shared = root.0.join("shared");
        let audio = root.0.join("audio");
        for directory in [&primary, &shared, &audio] {
            std::fs::create_dir(directory)?;
        }
        for directory in [&primary, &shared] {
            std::fs::write(directory.join("tokens.txt"), b"<blk> 0\nA 1\nB 2\n")?;
        }
        std::fs::write(shared.join("bpe.model"), sentencepiece(b'A'))?;
        assert!(!primary.join("bpe.model").exists());
        let exported = vocabulary(&primary, &audio, Some(&shared))?;
        assert_eq!(std::fs::read_to_string(&exported)?, "A\t0");

        std::fs::write(primary.join("bpe.model"), sentencepiece(b'B'))?;
        vocabulary(&primary, &audio, Some(&shared))?;
        assert_eq!(std::fs::read_to_string(&exported)?, "A\t0");

        vocabulary(&primary, &audio, None)?;
        assert_eq!(std::fs::read_to_string(&exported)?, "B\t0");
        std::fs::write(primary.join("tokens.txt"), b"different inventory\n")?;
        assert!(vocabulary(&primary, &audio, Some(&shared)).is_err());

        // The same size ceiling applies before parsing either tokenizer source.
        std::fs::File::create(primary.join("bpe.model"))?.set_len(2_000_001)?;
        let oversized = vocabulary(&primary, &audio, None).unwrap_err();
        assert_eq!(oversized.to_string(), "Tokenizer model too large");
        assert_eq!(std::fs::read_to_string(&exported)?, "B\t0");
        Ok(())
    }
}
