//! Equivalent integer money spellings for prompts and numeric evidence checks.
//! Keep unsupported/decimal expressions intact instead of guessing a value.
use regex::{Captures, Regex};
use std::sync::OnceLock;

fn small(word: &str) -> Option<u64> {
    [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ]
    .iter()
    .position(|w| *w == word)
    .map(|i| i as u64)
    .or_else(|| {
        [
            "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
        ]
        .iter()
        .position(|w| *w == word)
        .map(|i| (i as u64 + 2) * 10)
    })
}
fn below_hundred(words: &[&str]) -> Option<u64> {
    match words {
        [one] => small(one),
        [tens, units] => {
            let (tens, units) = (small(tens)?, small(units)?);
            (tens >= 20 && tens % 10 == 0 && (1..10).contains(&units)).then_some(tens + units)
        }
        _ => None,
    }
}
fn group(words: &[&str]) -> Option<u64> {
    if let [one, "hundred", rest @ ..] = words {
        let n = if *one == "a" { 1 } else { small(one)? };
        if !(1..10).contains(&n) {
            return None;
        }
        if rest.is_empty() {
            return Some(n * 100);
        }
        let rest = rest.strip_prefix(&["and"]).unwrap_or(rest);
        return Some(n * 100 + below_hundred(rest)?);
    }
    below_hundred(words)
}
fn english(text: &str) -> Option<u64> {
    let text = text.to_ascii_lowercase();
    let words: Vec<_> = text.split([' ', '-']).filter(|w| !w.is_empty()).collect();
    let (mut start, mut total, mut previous) = (0, 0u64, u64::MAX);
    for (i, word) in words.iter().enumerate() {
        let scale = match *word {
            "thousand" => 1000,
            "million" => 1_000_000,
            _ => continue,
        };
        if scale >= previous {
            return None;
        }
        let part = &words[start..i];
        let n = if part == ["a"] { 1 } else { group(part)? };
        total = total.checked_add(n.checked_mul(scale)?)?;
        previous = scale;
        start = i + 1;
    }
    let rest = &words[start..];
    if rest.is_empty() && start > 0 {
        return Some(total);
    }
    let rest = if start > 0 {
        rest.strip_prefix(&["and"]).unwrap_or(rest)
    } else {
        rest
    };
    total.checked_add(group(rest)?)
}
fn chinese(text: &str) -> Option<u64> {
    if let Some((high, low)) = text.split_once('万') {
        let high = super::validation::chinese_number(high)? as u64;
        let low = if low.is_empty() {
            0
        } else {
            super::validation::chinese_number(low)? as u64
        };
        Some(high * 10_000 + low)
    } else {
        super::validation::chinese_number(text).map(u64::from)
    }
}
pub(crate) fn normalize(text: &str) -> String {
    static EN: OnceLock<Regex> = OnceLock::new();
    static ZH: OnceLock<Regex> = OnceLock::new();
    let words = "zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred|thousand|million|billion|and|point|half|a";
    let en = EN.get_or_init(|| {
        Regex::new(&format!(
            r"(?i)\b(?P<n>(?:{words})(?:[ -]+(?:{words}))*)\s+(?P<u>dollars?|yuan|euros?|pounds?)\b"
        ))
        .unwrap()
    });
    let result = en.replace_all(text, |c: &Captures<'_>| {
        english(&c["n"])
            .map(|n| format!("{n} {}", &c["u"]))
            .unwrap_or_else(|| c[0].to_owned())
    });
    let zh = ZH.get_or_init(|| Regex::new(r"(?P<n>[0-9零〇一二两三四五六七八九十百千万亿点]+)(?P<u>美元|美金|人民币|欧元|英镑|元)").unwrap());
    zh.replace_all(&result, |c: &Captures<'_>| {
        chinese(&c["n"])
            .map(|n| format!("{n}{}", &c["u"]))
            .unwrap_or_else(|| c[0].to_owned())
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integer_money_spelling_never_changes_the_order_of_magnitude() {
        assert_eq!(
            normalize("THREE THOUSAND DOLLARS; 三千美元; 三万美元"),
            "3000 DOLLARS; 3000美元; 30000美元"
        );
        assert_eq!(
            normalize("twenty-three thousand five hundred yuan; 一万三千元"),
            "23500 yuan; 13000元"
        );
        for text in [
            "three point five dollars",
            "one and one dollars",
            "three billion dollars",
            "三点五万元",
            "3千元",
            "OmaScribe 三千次测试",
        ] {
            assert_eq!(normalize(text), text);
        }
    }
}
