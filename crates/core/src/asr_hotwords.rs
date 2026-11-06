//! The bounded UTF-8 wire format shared by note-context producers and ASR.
//!
//! Display spellings stay in terminology. This module prepares complete uppercase
//! hints for the decoder without clipping a name, a model number, or a UTF-8 scalar.
use anyhow::Result;
use serde::Serialize;
use std::collections::HashSet;

pub const MAX_WORD_BYTES: usize = 16_000;
pub const MAX_WORDS: usize = 80;
pub const MAX_WORD_CHARS: usize = 80;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DroppedWords {
    pub empty: usize,
    pub invalid_characters: usize,
    pub too_many_characters: usize,
    pub duplicate: usize,
    pub term_limit: usize,
    pub byte_limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HotwordLimits {
    pub max_word_bytes: usize,
    pub max_words: usize,
    pub max_word_chars: usize,
}

impl Default for HotwordLimits {
    fn default() -> Self {
        Self {
            max_word_bytes: MAX_WORD_BYTES,
            max_words: MAX_WORDS,
            max_word_chars: MAX_WORD_CHARS,
        }
    }
}

/// Counts and limits only: diagnostics must not expose the user's note terms.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HotwordStats {
    pub input_terms: usize,
    pub accepted_terms: usize,
    pub wire_bytes: usize,
    pub dropped_by_reason: DroppedWords,
    pub limits: HotwordLimits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedWords {
    pub words: String,
    pub stats: HotwordStats,
}

fn within_character_limit(word: &str) -> bool {
    word.chars().take(MAX_WORD_CHARS + 1).count() <= MAX_WORD_CHARS
}

fn valid_characters(word: &str) -> bool {
    word.chars().all(|c| c.is_alphanumeric() || c == ' ')
}

/// Validate an untrusted decoder input; malformed input is rejected as a whole.
/// Keep the decoder's existing normalization of trimmed, nonempty lines.
pub fn validate_words(raw: &str) -> Result<String> {
    anyhow::ensure!(raw.len() <= MAX_WORD_BYTES, "Hotword input too large");
    let words: Vec<_> = raw
        .lines()
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .collect();
    anyhow::ensure!(
        words.len() <= MAX_WORDS
            && words
                .iter()
                .all(|word| within_character_limit(word) && valid_characters(word)),
        "Invalid hotword input"
    );
    Ok(words.join("\n"))
}

/// Pack complete hints in their existing priority order, after Unicode casing.
/// A nonfitting hint is skipped so a later complete short name can still fit.
pub fn encode_words(candidates: &[String]) -> EncodedWords {
    let mut words = String::new();
    // Only accepted words enter the set, so auxiliary storage is bounded by 80.
    let mut seen = HashSet::new();
    let mut stats = HotwordStats {
        input_terms: candidates.len(),
        ..Default::default()
    };
    for candidate in candidates {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            stats.dropped_by_reason.empty += 1;
            continue;
        }
        // Bound the case-conversion allocation as well as its output. The
        // terminology producer already applies this same pre-casing limit.
        if !within_character_limit(candidate) {
            stats.dropped_by_reason.too_many_characters += 1;
            continue;
        }
        let word = candidate.to_uppercase();
        if !within_character_limit(&word) {
            stats.dropped_by_reason.too_many_characters += 1;
            continue;
        }
        if !valid_characters(&word) {
            stats.dropped_by_reason.invalid_characters += 1;
            continue;
        }
        if seen.contains(&word) {
            stats.dropped_by_reason.duplicate += 1;
            continue;
        }
        if stats.accepted_terms == MAX_WORDS {
            stats.dropped_by_reason.term_limit += 1;
            continue;
        }
        let separator_bytes = usize::from(!words.is_empty());
        if words.len() + separator_bytes + word.len() > MAX_WORD_BYTES {
            stats.dropped_by_reason.byte_limit += 1;
            continue;
        }
        if separator_bytes != 0 {
            words.push('\n');
        }
        words.push_str(&word);
        seen.insert(word);
        stats.accepted_terms += 1;
    }
    stats.wire_bytes = words.len();
    EncodedWords { words, stats }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    fn han_words(count: usize, supplementary: bool) -> Vec<String> {
        let (repeated, first) = if supplementary {
            ('𠀀', 0x20000)
        } else {
            ('界', 0x4e00)
        };
        (0..count)
            .map(|index| {
                format!(
                    "{}{}",
                    repeated.to_string().repeat(MAX_WORD_CHARS - 1),
                    char::from_u32(first + index as u32).unwrap()
                )
            })
            .collect()
    }

    #[test]
    fn decoder_normalization_and_native_syntax_rejection_stay_strict() {
        assert_eq!(
            validate_words(" OMASCRIBE \n\n转写\r\n").unwrap(),
            "OMASCRIBE\n转写"
        );
        assert_eq!(validate_words(" \n").unwrap(), "");
        for raw in [
            "A\0B".to_owned(),
            "NAME :99999".to_owned(),
            "ONE/TWO".to_owned(),
            "A\tB".to_owned(),
            "X\n".repeat(MAX_WORDS + 1),
            "X".repeat(MAX_WORD_CHARS + 1),
        ] {
            assert!(validate_words(&raw).is_err());
        }
    }

    #[test]
    fn exact_byte_boundary_includes_separators_and_rejects_one_more_byte() {
        let mut input = han_words(66, false);
        input.push(format!("{}A", "界".repeat(31)));
        let exact = input.join("\n");
        assert_eq!(exact.len(), MAX_WORD_BYTES);
        assert_eq!(validate_words(&exact).unwrap(), exact);
        assert!(validate_words(&(exact.clone() + "A")).is_err());
        let encoded = encode_words(&input);
        assert_eq!(encoded.words, exact);
        assert_eq!(encoded.stats.wire_bytes, MAX_WORD_BYTES);
        input.last_mut().unwrap().push('A');
        let clipped = encode_words(&input);
        assert_eq!(clipped.stats.accepted_terms, 66);
        assert_eq!(clipped.stats.dropped_by_reason.byte_limit, 1);
        assert_eq!(validate_words(&clipped.words).unwrap(), clipped.words);
    }

    #[test]
    fn aggregate_han_overflow_keeps_complete_terms_and_a_later_short_name() {
        let mut input = han_words(MAX_WORDS, false);
        // This was valid for the producer but caused the old consumer to lose
        // the entire context. Keep its exact previous wire shape as a regression.
        let previous_wire = input.join("\n").to_uppercase();
        assert_eq!(previous_wire.len(), 19_279);
        assert!(validate_words(&previous_wire).is_err());
        input.push("Bob".into());
        let encoded = encode_words(&input);
        let retained: Vec<_> = encoded.words.lines().collect();
        assert_eq!(retained.len(), 67);
        assert_eq!(retained[..66], input[..66]);
        assert_eq!(retained[66], "BOB");
        assert_eq!(encoded.stats.dropped_by_reason.byte_limit, 14);
        assert_eq!(encoded.stats.accepted_terms, 67);
        assert_eq!(encoded.stats.wire_bytes, 15_909);
        assert_eq!(validate_words(&encoded.words).unwrap(), encoded.words);
    }

    #[test]
    fn supplementary_letters_are_not_cut_inside_a_word_or_utf8_scalar() {
        let mut input = han_words(MAX_WORDS, true);
        assert_eq!(input.join("\n").len(), 25_679);
        input.push("Alice".into());
        let encoded = encode_words(&input);
        let retained: Vec<_> = encoded.words.lines().collect();
        assert_eq!(retained.len(), 50);
        assert_eq!(retained[..49], input[..49]);
        assert_eq!(retained[49], "ALICE");
        assert!(retained[..49].iter().all(|word| word.chars().count() == 80));
        assert_eq!(encoded.stats.dropped_by_reason.byte_limit, 31);
        assert_eq!(validate_words(&encoded.words).unwrap(), encoded.words);
    }

    #[test]
    fn uppercase_expansion_and_combining_marks_cannot_discard_valid_names() {
        let input = vec!["ß".repeat(40), "ß".repeat(41), "ΐ".into(), "Nvidia".into()];
        let encoded = encode_words(&input);
        assert_eq!(encoded.words, format!("{}\nNVIDIA", "S".repeat(80)));
        assert_eq!(encoded.stats.dropped_by_reason.too_many_characters, 1);
        assert_eq!(encoded.stats.dropped_by_reason.invalid_characters, 1);
        assert_eq!(validate_words(&encoded.words).unwrap(), encoded.words);
    }

    #[test]
    fn case_duplicates_and_rejected_terms_do_not_consume_accepted_capacity() {
        let input = candidates(&[
            " Alice ",
            "ALICE",
            "",
            "A\0B",
            "OmaScribe",
            "GTX 一六五零",
            "GTX ONE SIX FIVE ZERO",
        ]);
        let encoded = encode_words(&input);
        assert_eq!(
            encoded.words,
            "ALICE\nOMASCRIBE\nGTX 一六五零\nGTX ONE SIX FIVE ZERO"
        );
        assert_eq!(encoded.stats.input_terms, 7);
        assert_eq!(encoded.stats.accepted_terms, 4);
        assert_eq!(encoded.stats.dropped_by_reason.duplicate, 1);
        assert_eq!(encoded.stats.dropped_by_reason.empty, 1);
        assert_eq!(encoded.stats.dropped_by_reason.invalid_characters, 1);
        assert_eq!(validate_words(&encoded.words).unwrap(), encoded.words);
    }

    #[test]
    fn word_count_is_bounded_and_statistics_do_not_contain_note_text() {
        let input: Vec<_> = (0..MAX_WORDS + 1)
            .map(|index| format!("Term{index}"))
            .collect();
        let encoded = encode_words(&input);
        assert_eq!(encoded.stats.accepted_terms, MAX_WORDS);
        assert_eq!(encoded.stats.dropped_by_reason.term_limit, 1);
        assert_eq!(encoded.words.lines().last(), Some("TERM79"));
        assert_eq!(validate_words(&encoded.words).unwrap(), encoded.words);
        let metadata = serde_json::to_string(&encoded.stats).unwrap();
        assert!(!metadata.contains("Term") && !metadata.contains("TERM"));
        assert_eq!(encoded.stats.limits, HotwordLimits::default());
        assert_eq!(encode_words(&[]).stats, HotwordStats::default());
    }
}
