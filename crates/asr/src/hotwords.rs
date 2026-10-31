//! Complete hotword export and a model-specific necessary glyph check.
//! Forwarded hints have not been proven natively encodable. No hint is rewritten.
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_TOKENS_BYTES: u64 = 1_000_000;
const MAX_TOKENIZER_BYTES: u64 = 2_000_000;
const MAX_ENTRIES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    status: &'static str,
    reason: Option<&'static str>,
    submitted: usize,
    unsupported: usize,
    forwarded: usize,
    unsupported_lines: Vec<usize>,
}
impl Coverage {
    pub fn unavailable() -> Self {
        Self {
            status: "unavailable",
            reason: Some("inventory_unavailable"),
            submitted: 0,
            unsupported: 0,
            forwarded: 0,
            unsupported_lines: Vec::new(),
        }
    }
    pub fn diagnostics(&self) -> Value {
        json!({"schema":1,"method":"necessary_glyph_coverage","scope":"last_accepted_input",
            "status":self.status,"reason":self.reason,"submitted":self.submitted,
            "unsupported":self.unsupported,"forwarded":self.forwarded,
            "unsupported_lines":self.unsupported_lines,"line_ordinal_basis":"normalized_nonempty_wire",
            "native_encoding_verified":false})
    }
}

pub struct Prepared {
    pub words: String,
    pub coverage: Coverage,
    vocabulary: Option<Vocabulary>,
}
impl Prepared {
    pub fn unavailable() -> Self {
        Self {
            words: String::new(),
            coverage: Coverage::unavailable(),
            vocabulary: None,
        }
    }
    pub fn tokens(&self) -> Option<&[u8]> {
        self.vocabulary
            .as_ref()
            .map(|vocabulary| vocabulary.tokens.as_slice())
    }
    pub fn inventory_id(&self) -> Option<[u8; 32]> {
        self.vocabulary
            .as_ref()
            .map(|vocabulary| vocabulary.identity)
    }
    /// An unsupported-only edit must not create a stream or reload the model.
    pub fn changes_native_context(&self, words: &str, inventory: Option<[u8; 32]>) -> bool {
        self.words != words || !self.words.is_empty() && self.inventory_id() != inventory
    }
    pub fn export(&self, audio: &Path) -> Result<Option<PathBuf>> {
        if self.words.is_empty() {
            return Ok(None);
        }
        let vocabulary = self
            .vocabulary
            .as_ref()
            .context("Missing hotword inventory")?;
        let path = audio.join("bpe.vocab");
        // This happens only after preparation succeeds and native state is
        // actually changing. Parsing/refresh failures never touch this file.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let part = audio.join(format!(
            ".bpe-vocab-{}-{}.part",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut file = File::options().write(true).create_new(true).open(&part)?;
        let written = file.write_all(vocabulary.export.as_bytes());
        drop(file);
        let result = written.and_then(|()| fs::rename(&part, &path));
        if result.is_err() {
            let _ = fs::remove_file(&part);
        }
        result?;
        Ok(Some(path))
    }
}

struct Vocabulary {
    tokens: Vec<u8>,
    identity: [u8; 32],
    export: String,
    token_glyphs: HashSet<char>,
    bpe_glyphs: HashSet<char>,
    byte_fallback: bool,
    proven_inventory: bool,
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let canonical = fs::canonicalize(path)?;
    let metadata = fs::metadata(&canonical)?;
    ensure!(metadata.is_file(), "Invalid hotword inventory file");
    ensure!(
        metadata.len() <= limit,
        "{}",
        if limit == MAX_TOKENIZER_BYTES {
            "Tokenizer model too large"
        } else {
            "Token inventory too large"
        }
    );
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(&canonical)?;
    let before = file.metadata()?;
    ensure!(before.is_file(), "Invalid hotword inventory file");
    ensure!(
        before.len() <= limit,
        "{}",
        if limit == MAX_TOKENIZER_BYTES {
            "Tokenizer model too large"
        } else {
            "Token inventory too large"
        }
    );
    let mut bytes = Vec::new();
    (&file).take(limit + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    ensure!(!bytes.is_empty(), "Empty hotword inventory");
    ensure!(
        bytes.len() as u64 <= limit,
        "{}",
        if limit == MAX_TOKENIZER_BYTES {
            "Tokenizer model too large"
        } else {
            "Token inventory too large"
        }
    );
    ensure!(
        bytes.len() as u64 == before.len()
            && before.len() == after.len()
            && before.modified()? == after.modified()?
            && fs::canonicalize(path)? == canonical,
        "Hotword inventory changed while reading"
    );
    Ok(bytes)
}

fn varint(data: &[u8], cursor: &mut usize) -> Result<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(*cursor).context("Truncated SentencePiece model")?;
        *cursor += 1;
        ensure!(shift != 63 || byte <= 1, "Invalid SentencePiece varint");
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
        ensure!(tag >> 3 != 0, "Invalid SentencePiece field");
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
            ensure!(result.len() < MAX_ENTRIES, "Too many SentencePiece fields");
            result.push((tag >> 3, bytes));
        }
        cursor = end;
    }
    Ok(result)
}

fn ordinary(text: &str) -> bool {
    !text.starts_with(['<', '#'])
}
fn known_piece(text: &str) -> bool {
    matches!(text, "<blk>" | "<sos/eos>" | "<unk>")
        || text
            .strip_prefix('#')
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|c| c.is_ascii_digit()))
        || ordinary(text)
            && text
                .chars()
                .all(|c| c.is_ascii() || c == '▁' || ('\u{3400}'..='\u{9fff}').contains(&c))
}
fn add_glyphs(text: &str, glyphs: &mut HashSet<char>) {
    if ordinary(text) {
        glyphs.extend(text.chars().filter(|c| *c != '▁'));
    }
}

fn inventory(model: &Path, shared: Option<&Path>) -> Result<Vocabulary> {
    let tokens = read_bounded(&model.join("tokens.txt"), MAX_TOKENS_BYTES)?;
    let tokenizer = if let Some(shared) = shared {
        // The selected shared inventory is authoritative, including when a
        // model-local tokenizer appears later. Compare complete token bytes.
        let shared_tokens = read_bounded(&shared.join("tokens.txt"), MAX_TOKENS_BYTES)?;
        ensure!(
            tokens == shared_tokens,
            "Incompatible shared hotword tokenizer"
        );
        shared.join("bpe.model")
    } else {
        model.join("bpe.model")
    };
    let data = read_bounded(&tokenizer, MAX_TOKENIZER_BYTES)?;
    let mut token_glyphs = HashSet::new();
    let mut bpe_glyphs = HashSet::new();
    let mut token_names = HashSet::new();
    let mut token_ids = HashSet::new();
    let mut byte_fallback = false;
    let mut proven_inventory = true;
    for line in std::str::from_utf8(&tokens)?
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        ensure!(token_names.len() < MAX_ENTRIES, "Too many acoustic tokens");
        let mut fields = line.split_whitespace();
        let text = fields.next().context("Missing acoustic token")?;
        let id: i32 = fields
            .next()
            .context("Missing acoustic token ID")?
            .parse()?;
        ensure!(
            id >= 0 && fields.next().is_none() && token_names.insert(text) && token_ids.insert(id),
            "Invalid acoustic token inventory"
        );
        byte_fallback |= text.starts_with("<0x");
        proven_inventory &= known_piece(text);
        add_glyphs(text, &mut token_glyphs);
    }
    ensure!(!token_names.is_empty(), "Empty acoustic token inventory");
    let mut pieces = Vec::new();
    let mut piece_names = HashSet::new();
    for (_, piece) in fields(&data)?.into_iter().filter(|(field, _)| *field == 1) {
        let piece_fields = fields(piece)?;
        let text_fields: Vec<_> = piece_fields
            .iter()
            .filter(|(field, _)| *field == 1)
            .collect();
        ensure!(text_fields.len() == 1, "Invalid SentencePiece token");
        let text = std::str::from_utf8(text_fields[0].1)?;
        ensure!(
            !text.is_empty()
                && !text.chars().any(char::is_whitespace)
                && !text.chars().any(char::is_control)
                && piece_names.insert(text),
            "Invalid vocabulary token"
        );
        let scores: Vec<_> = piece_fields
            .iter()
            .filter(|(field, _)| *field == 2)
            .collect();
        ensure!(scores.len() <= 1, "Invalid vocabulary score");
        let score = match scores.first() {
            Some((_, value)) => {
                ensure!(value.len() == 4, "Invalid vocabulary score");
                f32::from_le_bytes((*value).try_into()?)
            }
            None => 0.,
        };
        ensure!(score.is_finite(), "Invalid vocabulary score");
        byte_fallback |= text.starts_with("<0x");
        proven_inventory &= known_piece(text);
        add_glyphs(text, &mut bpe_glyphs);
        pieces.push(format!("{text}\t{score}"));
    }
    ensure!(!pieces.is_empty(), "Empty tokenizer vocabulary");
    let mut hash = Sha256::new();
    hash.update(b"hotword-inventory-v1\0");
    hash.update((tokens.len() as u64).to_le_bytes());
    hash.update(&tokens);
    hash.update((data.len() as u64).to_le_bytes());
    hash.update(data);
    Ok(Vocabulary {
        tokens,
        identity: hash.finalize().into(),
        export: pieces.join("\n"),
        token_glyphs,
        bpe_glyphs,
        byte_fallback,
        proven_inventory,
    })
}

pub fn prepare(
    model: &Path,
    shared: Option<&Path>,
    raw: &str,
    known_scope: bool,
) -> Result<Prepared> {
    let normalized = omascribe_core::asr_hotwords::validate_words(raw)?;
    let lines: Vec<_> = normalized
        .lines()
        .enumerate()
        .filter_map(|(ordinal, line)| {
            let line = line.trim();
            (!line.is_empty()).then_some((ordinal, line))
        })
        .collect();
    let mut coverage = Coverage {
        status: "not_needed",
        reason: None,
        submitted: lines.len(),
        unsupported: 0,
        forwarded: 0,
        unsupported_lines: Vec::new(),
    };
    if lines.is_empty() {
        return Ok(Prepared {
            words: String::new(),
            coverage,
            vocabulary: None,
        });
    }
    let vocabulary = inventory(model, shared)?;
    coverage.status = "checked";
    coverage.reason = if !known_scope {
        Some("unproven_modeling_unit")
    } else if vocabulary.byte_fallback {
        Some("byte_fallback_inventory")
    } else if !vocabulary.proven_inventory {
        Some("unproven_inventory")
    } else {
        None
    };
    if coverage.reason.is_some() {
        coverage.status = "unknown";
    }
    let forwarded: Vec<_> = lines
        .into_iter()
        .filter_map(|(ordinal, line)| {
            let unsupported = coverage.status == "checked"
                && (line.chars().any(|c| c != ' ' && !vocabulary.token_glyphs.contains(&c))
                    // Native cjkchar+bpe applies BPE only to letter-leading
                    // ASCII runs. A standalone/digit-leading unit is direct.
                    || line.split(|c: char| !c.is_ascii_alphanumeric()).any(|word| {
                        word.starts_with(|c: char| c.is_ascii_alphabetic())
                            && word.chars().any(|c| !vocabulary.bpe_glyphs.contains(&c))
                    }));
            if unsupported {
                coverage.unsupported_lines.push(ordinal);
                None
            } else {
                Some(line)
            }
        })
        .collect();
    coverage.forwarded = forwarded.len();
    coverage.unsupported = coverage.unsupported_lines.len();
    Ok(Prepared {
        words: forwarded.join("\n"),
        coverage,
        vocabulary: Some(vocabulary),
    })
}

#[cfg(test)]
pub fn vocabulary(model: &Path, audio: &Path, shared: Option<&Path>) -> Result<PathBuf> {
    let vocabulary = inventory(model, shared)?;
    let path = audio.join("bpe.vocab");
    fs::write(&path, vocabulary.export)?;
    Ok(path)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct Scratch(pub(crate) PathBuf);

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

    pub(crate) fn coverage_fixture() -> Result<Scratch> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = Scratch(std::env::temp_dir().join(format!(
            "omascribe-glyph-coverage-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )));
        std::fs::create_dir(&root.0)?;
        std::fs::write(
            root.0.join("tokens.txt"),
            "<blk> 0\n<sos/eos> 1\n<unk> 2\n▁CHI 3\nN 4\nA 5\n测 6\n试 7\nF 8\n#1 9\n",
        )?;
        let mut model = Vec::new();
        for text in ["<blk>", "<sos/eos>", "<unk>", "▁CHI", "N", "A", "F"] {
            let mut piece = vec![10, text.len() as u8];
            piece.extend_from_slice(text.as_bytes());
            piece.push(21);
            piece.extend_from_slice(&0_f32.to_le_bytes());
            model.extend([10, piece.len() as u8]);
            model.extend(piece);
        }
        std::fs::write(root.0.join("bpe.model"), model)?;
        Ok(root)
    }

    #[test]
    fn necessary_coverage_rejects_complete_unsupported_lines_without_rewriting() -> Result<()> {
        let root = coverage_fixture()?;
        let prepared = prepare(
            &root.0,
            None,
            "CHINA\nF1\nCHIŞINĂU\nRECEP TAYYIP ERDOĞAN\nASUNCIÓN\n测试 CHINA",
            true,
        )?;
        assert_eq!(prepared.words, "CHINA\n测试 CHINA");
        let stats = prepared.coverage.diagnostics();
        assert_eq!(stats["status"], "checked");
        assert_eq!(stats["submitted"], 6);
        assert_eq!(stats["unsupported"], 4);
        assert_eq!(stats["forwarded"], 2);
        assert_eq!(stats["unsupported_lines"], serde_json::json!([1, 2, 3, 4]));
        assert_eq!(stats["native_encoding_verified"], false);
        assert!(!stats.to_string().contains("CHINA"));
        Ok(())
    }

    #[test]
    fn necessary_coverage_normalized_wire_ordinals_and_does_not_write_vocab() -> Result<()> {
        let root = coverage_fixture()?;
        let prepared = prepare(&root.0, None, "\n CHINA \nF1\n\nCHIŞINĂU\n", true)?;
        assert_eq!(prepared.words, "CHINA");
        assert_eq!(
            prepared.coverage.diagnostics()["unsupported_lines"],
            serde_json::json!([1, 2])
        );
        assert!(!root.0.join("bpe.vocab").exists());
        let empty = prepare(&root.0.join("missing"), None, "\n ", true)?;
        assert_eq!(empty.words, "");
        assert_eq!(empty.coverage.diagnostics()["status"], "not_needed");
        Ok(())
    }

    #[test]
    fn necessary_coverage_unknown_scope_does_not_claim_unsupported() -> Result<()> {
        let root = coverage_fixture()?;
        let unknown = prepare(&root.0, None, "F1\nASUNCIÓN", false)?;
        assert_eq!(unknown.words, "F1\nASUNCIÓN");
        assert_eq!(unknown.coverage.diagnostics()["status"], "unknown");
        assert_eq!(unknown.coverage.diagnostics()["unsupported"], 0);
        std::fs::write(
            root.0.join("tokens.txt"),
            "<blk> 0\n<sos/eos> 1\n<unk> 2\nA 3\n<0x00> 4\n",
        )?;
        let future = prepare(&root.0, None, "ASUNCIÓN", true)?;
        assert_eq!(future.words, "ASUNCIÓN");
        assert_eq!(future.coverage.diagnostics()["status"], "unknown");
        assert_eq!(
            future.coverage.diagnostics()["reason"],
            "byte_fallback_inventory"
        );
        Ok(())
    }

    #[test]
    fn necessary_coverage_rereads_inventory_instead_of_trusting_size_or_mtime() -> Result<()> {
        let root = coverage_fixture()?;
        let before = prepare(&root.0, None, "CHINA", true)?;
        assert_eq!(before.words, "CHINA");
        let path = root.0.join("tokens.txt");
        let time = std::fs::metadata(&path)?.modified()?;
        let content = std::fs::read_to_string(&path)?;
        std::fs::write(&path, content.replace("N 4", "X 4"))?;
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_times(std::fs::FileTimes::new().set_modified(time))?;
        let after = prepare(&root.0, None, "CHINA", true)?;
        assert_eq!(after.words, "");
        assert_ne!(before.inventory_id(), after.inventory_id());
        std::fs::write(&path, b"invalid inventory")?;
        assert!(prepare(&root.0, None, "CHINA", true).is_err());
        Ok(())
    }

    #[test]
    fn necessary_coverage_unsupported_only_edits_do_not_change_native_context() -> Result<()> {
        let root = coverage_fixture()?;
        let before = prepare(&root.0, None, "CHINA", true)?;
        let added = prepare(&root.0, None, "CHINA\nF1\nASUNCIÓN", true)?;
        assert!(!added.changes_native_context(&before.words, before.inventory_id()));
        assert_eq!(added.coverage.diagnostics()["submitted"], 3);
        assert_eq!(added.coverage.diagnostics()["unsupported"], 2);
        let only_unsupported = prepare(&root.0, None, "F1", true)?;
        assert!(!only_unsupported.changes_native_context("", None));
        assert!(only_unsupported.changes_native_context(&before.words, before.inventory_id()));
        // A real tokenizer change reloads even if the filtered text is equal.
        let path = root.0.join("bpe.model");
        let mut bytes = std::fs::read(&path)?;
        let last = bytes.last_mut().unwrap();
        *last = 0x3f; // finite score with different bits, same inventory size
        std::fs::write(&path, bytes)?;
        let changed = prepare(&root.0, None, "CHINA", true)?;
        assert_eq!(changed.words, before.words);
        assert!(changed.changes_native_context(&before.words, before.inventory_id()));
        Ok(())
    }

    #[test]
    fn necessary_coverage_does_not_require_bpe_for_native_direct_numeric_units() -> Result<()> {
        let root = coverage_fixture()?;
        let path = root.0.join("tokens.txt");
        let mut tokens = std::fs::read_to_string(&path)?;
        tokens.push_str("2 10\n");
        std::fs::write(path, tokens)?;
        // cjkchar+bpe sends a digit-leading ASCII unit directly to the symbol
        // table. It does not ask the BPE encoder for a standalone2 piece.
        let direct = prepare(&root.0, None, "CHINA 2", true)?;
        assert_eq!(direct.words, "CHINA 2");
        assert_eq!(direct.coverage.diagnostics()["unsupported"], 0);
        // A letter-leading unit does require BPE character coverage.
        assert_eq!(prepare(&root.0, None, "F2", true)?.words, "");
        Ok(())
    }
}
