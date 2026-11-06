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

/// A bare amount does not establish a currency. This is a targeted guard for
/// supported currency spellings, not an exchange-rate or general unit parser.
fn currency_units() -> &'static [(&'static str, Regex)] {
    static UNITS: OnceLock<Vec<(&str, Regex)>> = OnceLock::new();
    UNITS.get_or_init(|| {
        [
            ("dollar", r"(?i)\b(?:usd|dollars?)\b|美元|美金|\$"),
            (
                "yuan",
                r"(?i)\b(?:cny|rmb|yuan)\b|人民币|[0-9零〇一二两三四五六七八九十百千万亿]+\s*元",
            ),
            ("euro", r"(?i)\b(?:eur|euros?)\b|欧元|€"),
            ("pound", r"(?i)\b(?:gbp|pounds?)\b|英镑|£"),
        ]
        .into_iter()
        .map(|(name, pattern)| (name, Regex::new(pattern).unwrap()))
        .collect()
    })
}
pub(super) fn added_currency(body: &str, evidence: &str) -> Option<&'static str> {
    currency_units()
        .iter()
        .find(|(_, unit)| unit.is_match(body) && !unit.is_match(evidence))
        .map(|(name, _)| *name)
}

fn currency_evidence(text: &str) -> bool {
    static OTHER: OnceLock<Regex> = OnceLock::new();
    currency_units().iter().any(|(_, unit)| unit.is_match(text))
        || text.contains(['¥', '￥', '₹', '₽', '₩', '¢'])
        || OTHER.get_or_init(|| Regex::new(r"(?i)\b(?:jpy|yen|hkd|cad|aud|chf|inr|krw|rub|currency|denominated|payable|kroner|krona|francs?|rupees?|rubles?)\b|日元|港元|港币|港幣|台币|臺幣|韩元|韓元|澳元|加元|法郎|卢布|卢比|币种|幣種|结算|結算").unwrap()).is_match(text)
}
fn plain_scalar(text: &str, generated: bool) -> Option<&str> {
    static DIGITS: OnceLock<Regex> = OnceLock::new();
    static COMPLEX: OnceLock<Regex> = OnceLock::new();
    if COMPLEX.get_or_init(|| Regex::new(r"(?i)\b(?:between|about|around|approximately|hundred|thousand|million|billion|year|date|month|version|build|id|number)\b|\b(?:up\s+to|at\s+least|at\s+most|more\s+than|less\s+than)\b|至少|最多|最少|不超过|不低于|以上|以下|左右|大约|约为|区间|范围|[≤≥<>]|[0-9]\s*[:：]\s*[0-9]").unwrap()).is_match(text) {
        return None;
    }
    let mut scalar = None;
    for m in DIGITS
        .get_or_init(|| Regex::new(r"[0-9]+").unwrap())
        .find_iter(text)
    {
        let value = m.as_str();
        if value.starts_with('0')
            || value.parse::<u64>().ok().is_none_or(|n| n == 0)
            || scalar.is_some_and(|n| n != value)
        {
            return None;
        }
        let before = &text[..m.start()];
        let after = &text[m.end()..];
        if before
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || ".+-−±~～–—*/^×÷=".contains(c))
            || before.trim_end().ends_with([
                '-', '+', '−', '±', '~', '～', '–', '—', '*', '/', '^', '×', '÷', '=',
            ])
            || before
                .strip_suffix(',')
                .is_some_and(|s| s.ends_with(|c: char| c.is_ascii_digit()))
            || after.starts_with('.') && after[1..].starts_with(|c: char| c.is_ascii_digit())
            || after.starts_with(',') && after[1..].starts_with(|c: char| c.is_ascii_digit())
        {
            return None;
        }
        let mut rest = after.trim_start();
        if generated && rest.starts_with('元') {
            rest = &rest['元'.len_utf8()..];
        }
        let tail = rest.trim_start();
        if tail.starts_with(|c: char| {
            "万千百亿年月日号天时分秒个条次人%‰+-−±~～–—*/^×÷=:：".contains(c)
        }) {
            return None;
        }
        if tail.starts_with(|c: char| c.is_alphanumeric())
            && ![
                "尚未", "未", "还没", "没有", "仍", "已", "不", "待", "的", "是", "为",
            ]
            .iter()
            .any(|s| tail.starts_with(s))
            && ![
                "is",
                "was",
                "has",
                "have",
                "had",
                "still",
                "remains",
                "not",
                "unapproved",
                "pending",
                "and",
                "but",
                "yet",
            ]
            .contains(
                &tail
                    .split(|c: char| !c.is_ascii_alphabetic())
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str(),
            )
        {
            return None;
        }
        scalar = Some(value);
    }
    scalar
}
/// Remove only a generated yuan suffix when one exact, plainly unitless money
/// value is known. This never supplies a currency or repairs a different value;
/// ambiguous output stays unchanged for the ordinary validators to reject.
pub(crate) fn repair_unitless_yuan(body: &str, source: &str) -> String {
    static MONEY: OnceLock<Regex> = OnceLock::new();
    static NONMONEY: OnceLock<Regex> = OnceLock::new();
    if !MONEY.get_or_init(|| Regex::new(r"(?i)(?:\b(?:budget(?:\s+amount)?|money|amount|cost|price)\b|预算(?:金额)?|金额|款项|价钱|价格|费用|钱款)\s*[:：]?\s*(?:(?:is|was|of|at|remains|still|set|to|为|是|按|先按|暂按|仍为|仍是|改为|调整为)\s*){0,8}[0-9]").unwrap()).is_match(source)
        || NONMONEY.get_or_init(|| Regex::new(r"(?i)\b(?:memory|ram|vram|tokens?|cpu|gpu|latency|time|storage|disk)\s+(?:budget|cost|amount)\b|(?:内存|显存|存储|时长|时间|延迟|令牌|调用次数|请求次数|字数)(?:预算|金额|开销|成本)").unwrap()).is_match(source)
        || currency_evidence(source)
    {
        return body.into();
    }
    let Some(value) = plain_scalar(source, false) else {
        return body.into();
    };
    if plain_scalar(body, true) != Some(value) {
        return body.into();
    }
    // The shared currency parser identifies numeric 元. Delete that character
    // only; surrounding digits, whitespace, punctuation and status are intact.
    let mut result = String::new();
    let mut copied = 0;
    for (at, _) in body.match_indices(value) {
        let end = at + value.len();
        let rest = &body[end..];
        let unit = end + rest.len() - rest.trim_start().len();
        if body[unit..].starts_with('元') {
            result.push_str(&body[copied..unit]);
            copied = unit + '元'.len_utf8();
        }
    }
    result.push_str(&body[copied..]);
    if currency_evidence(&result) {
        body.into()
    } else {
        result
    }
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
    #[test]
    fn unsupported_yuan_is_removed_only_from_one_explicit_unitless_money_value() {
        let source = "Morgan owns the budget review. The amount of 3000 is not approved yet. The review deadline is Thursday.";
        let body = "Morgan负责预算审核。3000元尚未批准。审核截止日期为周四。";
        let fixed = repair_unitless_yuan(body, source);
        assert_eq!(
            fixed,
            "Morgan负责预算审核。3000尚未批准。审核截止日期为周四。"
        );
        assert_eq!(added_currency(body, source), Some("yuan"));
        assert_eq!(added_currency(&fixed, source), None);
        assert_eq!(
            repair_unitless_yuan(
                "预算为3000 元，仍未批准。",
                "预算为3000，金额仍是3000，尚未批准。"
            ),
            "预算为3000 ，仍未批准。"
        );
        assert_eq!(
            repair_unitless_yuan("预算3000元。金额仍为3000元。", "预算3000。金额仍为3000。"),
            "预算3000。金额仍为3000。"
        );
    }
    #[test]
    fn ambiguous_or_nonmoney_values_keep_the_original_currency_guard() {
        let body = "预算3000元尚未批准。";
        for source in [
            "预算3000元尚未批准。",
            "The budget is 3000 yuan.",
            "The budget is 3000 USD.",
            "The budget is 3000USD.",
            "The budget is ¥3000.",
            "预算3000，以日元结算。",
            "预算3000，发票4000。",
            "The budget is 3000. The invoice is 4000.",
            "预算3000.5。",
            "预算3,000。",
            "预算3000万。",
            "预算3 thousand。",
            "预算3000到4000。",
            "The budget is between 3000 and 3000.",
            "预算至少3000。",
            "The budget is zero.",
            "预算0。",
            "预算03000。",
            "预算-3000。",
            "会议日期为3000年。",
            "预算会议日期为3000年。",
            "The budget review meets in room 3000.",
            "内存预算3000。",
            "The memory budget is 3000.",
            "The token amount is 3000.",
            "The amount of 3000 samples is recorded.",
            "测试3000个单元。",
        ] {
            assert_eq!(repair_unitless_yuan(body, source), body, "{source}");
        }
        for body in [
            "预算4000元。",
            "预算3000元及4000元。",
            "预算3000.5元。",
            "预算3,000元。",
            "预算3000万元。",
            "预算3000元至3000元。",
            "预算约为3000元。",
            "预算3000元和USD。",
            "预算3000元和yuan。",
            "预算3000元，日期为3000年。",
            "3000元数据待处理。",
            "3000元件待检查。",
            "单元测试与元数据。",
            "元。",
        ] {
            assert_eq!(repair_unitless_yuan(body, "预算3000。"), body, "{body}");
        }
        assert_eq!(added_currency("预算4000元。", "预算3000。"), Some("yuan"));
        assert_eq!(
            added_currency("预算3000美元。", "预算3000。"),
            Some("dollar")
        );
    }
    #[test]
    fn arithmetic_ratios_and_clocks_are_not_plain_money_scalars() {
        for source in [
            "The budget is 3*3.",
            "The budget is 3 × 3.",
            "The budget is 3000/3000.",
            "预算3000 ÷ 3000。",
            "预算3^3。",
            "预算3=3。",
            "预算3000:3000。",
            "预算3000：3000。",
            "预算3000，时钟为30:30。",
        ] {
            let body = if source.contains("3000") {
                "预算3000元。"
            } else {
                "预算3元。"
            };
            assert_eq!(repair_unitless_yuan(body, source), body, "{source}");
        }
        for body in [
            "预算3000元/3000元。",
            "预算3000元 : 3000元。",
            "预算3000元×3000元。",
            "预算3000元=3000元。",
        ] {
            assert_eq!(repair_unitless_yuan(body, "预算3000。"), body, "{body}");
        }
        assert_eq!(
            repair_unitless_yuan("预算3000元，尚未批准。", "budget:3000, not approved."),
            "预算3000，尚未批准。"
        );
        assert_eq!(
            repair_unitless_yuan("预算3000元。", "预算：3000。"),
            "预算3000。"
        );
    }
}
