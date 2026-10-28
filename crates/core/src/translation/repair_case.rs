//! Render all-caps grammatical English for a name-masked repair prompt only.
//! Source spellings live behind exact local IDs; the original speech and its
//! validation evidence are never rewritten by this helper.

/// Normalize only recognizably grammatical, entirely uppercase ASCII prose.
/// Mixed-script speech, labels and unrecognized bracket syntax stay literal.
pub(super) fn normalize(input: &str) -> String {
    if input
        .chars()
        .any(|c| c.is_ascii_lowercase() || c.is_alphabetic() && !c.is_ascii())
    {
        return input.into();
    }
    let mut spans = Vec::new();
    let mut at = 0;
    while at < input.len() {
        let rest = &input[at..];
        let c = rest.chars().next().unwrap();
        if c == '[' {
            let bytes = rest.as_bytes();
            if bytes.len() < 3 || !(b'A'..=b'L').contains(&bytes[1]) || bytes[2] != b']' {
                return input.into();
            }
            at += 3;
        } else if c == ']' {
            return input.into();
        } else if token_char(c) {
            let start = at;
            at += c.len_utf8();
            while at < input.len() {
                let c = input[at..].chars().next().unwrap();
                if !token_char(c) {
                    break;
                }
                at += c.len_utf8();
            }
            spans.push(start..at);
        } else {
            at += c.len_utf8();
        }
    }

    // Do not treat a bare brand/acronym or a joined list of labels as prose.
    // A finite/modal auxiliary plus another grammatical word is a bounded
    // cue, not a general English parser or an identity resolver.
    const AUXILIARIES: &[&str] = &[
        "AM", "IS", "ARE", "WAS", "WERE", "BE", "BEEN", "BEING", "HAS", "HAVE", "HAD", "DO",
        "DOES", "DID", "WILL", "WOULD", "SHALL", "SHOULD", "CAN", "COULD", "MAY", "MIGHT", "MUST",
    ];
    const FUNCTION_WORDS: &[&str] = &[
        "A", "AN", "THE", "I", "WE", "YOU", "HE", "SHE", "IT", "THEY", "THIS", "THAT", "THESE",
        "THOSE", "NOT", "AND", "OR", "BUT", "IF", "THEN", "TO", "OF", "FOR", "ON", "IN", "AT",
        "BY", "WITH", "FROM", "AS",
    ];
    let words: Vec<_> = spans.iter().map(|range| &input[range.clone()]).collect();
    if !words.iter().any(|word| AUXILIARIES.contains(word))
        || words
            .iter()
            .filter(|word| AUXILIARIES.contains(word) || FUNCTION_WORDS.contains(word))
            .count()
            < 2
    {
        return input.into();
    }
    let mut output = String::with_capacity(input.len());
    let mut end = 0;
    for range in spans {
        output.push_str(&input[end..range.start]);
        let word = &input[range.clone()];
        // Model/device/version labels are not ordinary prose. Preserve their
        // case even when a neighboring grammatical sentence is normalized.
        if word
            .chars()
            .any(|c| c.is_ascii_digit() || matches!(c, '_' | '-'))
        {
            output.push_str(word);
        } else {
            output.push_str(&word.to_ascii_lowercase());
        }
        end = range.end;
    }
    output.push_str(&input[end..]);
    output
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '\'' | '’')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammatical_case_changes_only_prompt_prose_outside_exact_name_ids() {
        let input = "IT HAS NOT BEEN APPROVED [B] WILL CHECK THE [A]";
        assert_eq!(
            normalize(input),
            "it has not been approved [B] will check the [A]"
        );
        assert_eq!(
            normalize("[A] WILL TEST THE [L] ON THURSDAY."),
            "[A] will test the [L] on thursday."
        );
    }

    #[test]
    fn identifiers_numbers_and_utf8_punctuation_keep_exact_bytes() {
        assert_eq!(
            normalize("[A] WILL TEST GTX1650 AND GPU_ID AND MODEL-Q4 — THE BUDGET IS 3000."),
            "[A] will test GTX1650 and GPU_ID and MODEL-Q4 — the budget is 3000."
        );
        assert_eq!(
            normalize("IT HAS 2048 TOKENS; [A][B] WILL TEST MODEL_V2."),
            "it has 2048 tokens; [A][B] will test MODEL_V2."
        );
    }

    #[test]
    fn mixed_case_scripts_labels_and_symbols_are_not_normalized() {
        for input in [
            "It HAS NOT BEEN APPROVED [B] WILL CHECK THE [A]",
            "已经批准 [A] WILL CHECK THE GPU",
            "ÉTAT IS APPROVED [A]",
            "CPU",
            "NASA API",
            "NEW YORK AND LOS ANGELES",
            "THE [A] AND THE [B]",
            "[A][L] 3000 — !",
            "[A]",
            "",
        ] {
            assert_eq!(normalize(input), input);
        }
    }

    #[test]
    fn malformed_or_nonlocal_markers_stay_literal() {
        for input in [
            "IT HAS NOT [M] WILL TEST THE [A]",
            "IT HAS NOT [API] WILL TEST THE [A]",
            "IT HAS NOT [A WILL TEST THE GPU",
            "IT HAS NOT A] WILL TEST THE GPU",
        ] {
            assert_eq!(normalize(input), input);
        }
    }
}
