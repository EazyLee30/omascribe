use omascribe_core::{library::Entry, terminology::Terms, UserNote};

fn note(content: &str) -> UserNote {
    UserNote {
        id: "test-note".into(),
        meeting_id: "test-meeting".into(),
        content: content.into(),
        kind: "document".into(),
        anchor_ms: 0,
        updated_at: String::new(),
    }
}

fn glossary_term(title: &str, aliases: &str) -> Entry {
    Entry {
        kind: "term".into(),
        title: title.into(),
        aliases: aliases.into(),
        ..Default::default()
    }
}

#[test]
fn standalone_model_keeps_all_digits_and_accepts_spaced_or_compact_spelling() {
    let spaced = Terms::new(&[note("GTX 1650")], &[]);
    assert!(spaced.hotwords.iter().any(|term| term == "GTX 1650"));
    assert!(spaced
        .recognition_hotwords()
        .iter()
        .any(|term| term == "GTX 一六五零"));
    assert!(spaced
        .recognition_hotwords()
        .iter()
        .any(|term| term == "GTX ONE SIX FIVE ZERO"));
    assert!(!spaced
        .recognition_hotwords()
        .iter()
        .any(|term| term == "GTX 1650"));
    assert_eq!(spaced.normalize("gtx1650"), "GTX 1650");
    assert_eq!(spaced.normalize("GTX 1650"), "GTX 1650");
    assert_eq!(spaced.normalize("GTX1650 一些功能"), "GTX 1650 一些功能");

    let compact = Terms::new(&[note("gtx1650")], &[]);
    assert!(compact.hotwords.iter().any(|term| term == "gtx1650"));
    assert!(compact
        .recognition_hotwords()
        .iter()
        .any(|term| term == "gtx 一六五零"));
    assert_eq!(compact.normalize("GTX 1650"), "gtx1650");
    assert_eq!(compact.normalize("GTX1650"), "gtx1650");

    // The meeting editor stores Markdown, including list and bold markers.
    let markdown = Terms::new(&[note("- **GTX 1650**\n\n后续测试")], &[]);
    assert_eq!(markdown.normalize("GTX1650"), "GTX 1650");
}

#[test]
fn chinese_digit_spelling_requires_the_entire_model_prefix_and_number() {
    let terms = Terms::new(&[note("GTX 1650")], &[]);
    assert_eq!(
        terms.normalize("测试 GTX 一六五零 的功能"),
        "测试 GTX 1650 的功能"
    );
    assert_eq!(
        terms.normalize("测试 GTX 一六五〇 的功能"),
        "测试 GTX 1650 的功能"
    );
    assert_eq!(
        terms.normalize("测试 GTX ONE SIX FIVE ZERO 的功能"),
        "测试 GTX 1650 的功能"
    );
    assert_eq!(
        terms.normalize("测试 GTX 一六六零 的功能"),
        "测试 GTX 一六六零 的功能"
    );
    assert_eq!(
        terms.normalize("测试 RTX 一六五零 的功能"),
        "测试 RTX 一六五零 的功能"
    );
    assert_eq!(
        terms.normalize("测试 GT 一六五零 的功能"),
        "测试 GT 一六五零 的功能"
    );
    assert_eq!(
        terms.normalize("测试 GTX 一六五 的功能"),
        "测试 GTX 一六五 的功能"
    );
}

#[test]
fn minutes_must_keep_the_complete_explicit_model_when_it_is_in_the_source() {
    let terms = Terms::new(&[note("GTX 1650")], &[]);
    assert!(terms
        .validate_writer_names("GTX 1650 is ready", "GTX 1650 已准备好")
        .is_ok());
    assert!(terms
        .validate_writer_names("GTX1650 is ready", "GTX 1660 已准备好")
        .is_err());
    assert!(terms
        .validate_writer_names("GTX 一六五零已准备好", "GTX1650 已准备好")
        .is_ok());
    assert!(terms
        .validate_writer_names("GTX 一六五零已经准备好", "显卡已经准备好")
        .is_err());
    // A note cannot create an unspoken model requirement.
    assert!(terms
        .validate_writer_names("RTX 1660 is ready", "RTX 1660 已准备好")
        .is_ok());
}

#[test]
fn explicit_glossary_wins_and_conflicting_aliases_are_not_guessed() {
    let glossary = [glossary_term("GeForce GTX 1650", "GTX1650,GTX 1650")];
    let terms = Terms::new(&[note("gtx 1650")], &glossary);
    assert_eq!(terms.normalize("GTX1650"), "GeForce GTX 1650");
    assert_eq!(terms.normalize("GTX 1650"), "GeForce GTX 1650");
    assert_eq!(terms.normalize("GTX 一六五零"), "GeForce GTX 1650");
    assert!(terms
        .recognition_hotwords()
        .iter()
        .any(|term| term.eq_ignore_ascii_case("GTX 一六五零")));

    let ambiguous = [
        glossary_term("First GPU", "GTX1650"),
        glossary_term("Second GPU", "GTX1650"),
    ];
    let terms = Terms::new(&[note("GTX 1650")], &ambiguous);
    assert_eq!(terms.normalize("GTX1650"), "GTX1650");

    let priority = [
        glossary_term("Graphics", "GTX1650"),
        glossary_term("Order", "GTX 一六五零"),
    ];
    assert_eq!(
        Terms::new(&[], &priority).normalize("GTX 一六五零"),
        "Order"
    );
    assert_eq!(
        Terms::new(
            &[note("GTX 1650")],
            &[glossary_term("Order", "GTX 一六五零")]
        )
        .normalize("GTX 一六五零"),
        "Order"
    );
    let product_alias = Terms::new(
        &[note("OmaScribe")],
        &[glossary_term("Other Product", "奥马斯")],
    );
    assert_eq!(product_alias.normalize("奥马斯"), "Other Product");
    let several_models = [glossary_term("Graphics", "GTX1650, RTX3060")];
    let hints = Terms::new(&[], &several_models).recognition_hotwords();
    assert!(hints.iter().any(|s| s.eq_ignore_ascii_case("GTX 一六五零")));
    assert!(hints.iter().any(|s| s.eq_ignore_ascii_case("RTX 三零六零")));
}

#[test]
fn unrelated_models_extra_digits_amounts_and_missing_context_stay_unchanged() {
    let terms = Terms::new(&[note("GTX 1650")], &[]);
    for text in [
        "RTX 1650",
        "GTX 1660",
        "GT 1650",
        "GTX 16500",
        "GTX 1650Ti",
        "GTX 1650 Ti",
        "MY_GTX1650_CARD",
        "GTX 一六五零六",
        "GTX 一六五零2",
        "GTX 一六五零元",
        "GTX 一六五零.5",
        "GTX 一六五零点五",
        "GTX 一六五零 点五",
        "GTX 一 六 五 零 点 五",
        "GTX 一六五零点\t五",
        "GTX 一六五零. 5",
        "GTX 一六五零． 五",
        "GTX 一六五零.５",
        "GTX 一六五零５",
        "GTX 一六五零 ５",
        "GTX 一六五零 .5",
        "GTX 一六五零件",
        "GTX 一六五零公斤",
        "GTX 一六五零美元",
        "GTX 一六五零%",
        "GTX 一 六 五 零 六",
        "GTX ONE SIX FIVE ZERO SIX",
        "GTX ONE SIX FIVE ZERO POINT FIVE",
        "GTX ONE SIX FIVE ZERO DOLLARS",
        "GTX ONE SIX FIVE ZERO USD",
        "预算一六五零元",
        "会议安排在 1650 年",
    ] {
        assert_eq!(terms.normalize(text), text, "unexpected rewrite of {text}");
    }
    assert_eq!(terms.normalize("GTX 一 六 五 零 的功能"), "GTX 1650 的功能");
    assert_eq!(terms.normalize("GTX 一六五零 点我"), "GTX 1650 点我");
    assert_eq!(
        terms.normalize("GTX ONE ſIX FIVE ZERO"),
        "GTX ONE ſIX FIVE ZERO"
    );

    let no_model_note = Terms::new(&[note("讨论预算和进度")], &[]);
    assert_eq!(no_model_note.normalize("GTX 一六五零"), "GTX 一六五零");
    assert_eq!(Terms::new(&[], &[]).normalize("GTX1650"), "GTX1650");
    assert!(terms.spellings("MY_GTX1650_CARD").is_empty());
    for label in ["Budget 3000", "budget3000", "YEAR 2026", "RMB 3000"] {
        let ordinary = Terms::new(&[note(label)], &[]);
        assert_eq!(ordinary.normalize(label), label);
        assert!(!ordinary
            .recognition_hotwords()
            .iter()
            .any(|term| term.contains('零') || term.contains("ZERO")));
    }
    assert_eq!(
        terms.fingerprint(),
        Terms::new(&[note("GTX 1650")], &[]).fingerprint()
    );
}

#[test]
fn duplicate_model_lines_do_not_hide_later_models_or_names() {
    let content = format!("{}RTX 3060\nAlice", "GTX 1650\n".repeat(80));
    let terms = Terms::new(&[note(&content)], &[]);
    let hints = terms.recognition_hotwords();
    assert!(hints.iter().any(|s| s.eq_ignore_ascii_case("GTX 一六五零")));
    assert!(hints.iter().any(|s| s.eq_ignore_ascii_case("RTX 三零六零")));
    assert!(hints.iter().any(|s| s.eq_ignore_ascii_case("Alice")));
    assert!(hints.len() <= 80);
    assert_eq!(terms.normalize("RTX 三零六零"), "RTX 3060");
    let repeated = Terms::new(&[note(&format!("{}Alice", "Bob\n".repeat(80)))], &[]);
    assert!(repeated.recognition_hotwords().iter().any(|s| s == "Bob"));
    assert!(repeated.recognition_hotwords().iter().any(|s| s == "Alice"));

    let mut at_limit = vec![glossary_term("Graphics", "GTX1650")];
    at_limit.extend((0..79).map(|n| glossary_term(&format!("Term{n:02}"), "")));
    let hints = Terms::new(&[], &at_limit).recognition_hotwords();
    assert_eq!(hints.len(), 80);
    assert!(hints.iter().any(|s| s.eq_ignore_ascii_case("GTX 一六五零")));
}
