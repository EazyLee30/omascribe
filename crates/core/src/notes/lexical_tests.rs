//! Frozen full-vocabulary scorer and fictional regressions for lexical changes.
use super::*;
use crate::Segment;
use std::sync::atomic::Ordering;

// Independent tokenizer copied from retrieval.rs at c741a8a. A candidate
// tokenizer change must not silently change its comparison reference too.
fn legacy_tokens(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut latin = String::new();
    let mut previous = None;
    for c in text.to_lowercase().chars().chain(std::iter::once(' ')) {
        if ('\u{3400}'..='\u{9fff}').contains(&c) {
            if !latin.is_empty() {
                result.push(std::mem::take(&mut latin));
            }
            result.push(c.to_string());
            if let Some(p) = previous {
                result.push(format!("{p}{c}"));
            }
            previous = Some(c);
        } else {
            previous = None;
            if c.is_ascii_alphanumeric() {
                latin.push(c);
            } else if !latin.is_empty() {
                result.push(std::mem::take(&mut latin));
            }
        }
    }
    result
}

// Previous implementation's BM25/phrase rules, with the same deterministic term
// order as the candidate. Do not optimize this independent comparison path.
fn legacy(all: &[&str], query: &[String]) -> Vec<usize> {
    let documents: Vec<_> = all.iter().map(|s| legacy_tokens(s)).collect();
    let mut terms: Vec<_> = query
        .iter()
        .flat_map(|q| legacy_tokens(q))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    terms.sort_unstable();
    let average =
        documents.iter().map(Vec::len).sum::<usize>() as f64 / documents.len().max(1) as f64;
    let mut df = HashMap::<&str, usize>::new();
    for document in &documents {
        for word in document.iter().collect::<HashSet<_>>() {
            *df.entry(word.as_str()).or_default() += 1;
        }
    }
    let mut scores: Vec<_> = documents
        .iter()
        .enumerate()
        .map(|(i, document)| {
            let mut score = 0.;
            for term in &terms {
                let frequency = document.iter().filter(|t| *t == term).count() as f64;
                let hits = *df.get(term.as_str()).unwrap_or(&0) as f64;
                let idf = (1. + (documents.len() as f64 - hits + 0.5) / (hits + 0.5)).ln();
                score += idf * frequency * 2.2
                    / (frequency + 1.2 * (0.25 + 0.75 * document.len() as f64 / average.max(1.)));
            }
            let lower = all[i].to_lowercase();
            score += query
                .iter()
                .filter(|q| lower.contains(&q.to_lowercase()))
                .count() as f64
                * 3.;
            (i, score)
        })
        .filter(|(_, s)| *s > 0.)
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
    scores.into_iter().map(|(i, _)| i).collect()
}
fn segment(seq: u32, text: &str) -> Segment {
    Segment {
        id: format!("s{seq}"),
        meeting_id: "m".into(),
        run: 1,
        seq,
        start_ms: seq as u64 * 1000,
        end_ms: (seq as u64 + 1) * 1000,
        text: text.into(),
        revision: 1,
        speaker: None,
        speaker_manual: false,
    }
}
fn plain(all: &[Segment], query: &[String]) -> Vec<usize> {
    ranked_text(
        &all.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
        query,
    )
}
#[test]
fn query_counts_match_full_vocabulary_including_empty_and_tied_cases() {
    let texts = [
        "",
        "!!!",
        "张三未批准预算 3000，周五再议。",
        "张三已撤回预算。",
        "OmaScribe OFFLINE on Friday",
        "OmaScribe OFFLINE on Friday",
        "ÉÉ ß Ω 汉字\nabc123\u{a0}中文",
    ];
    let queries = [
        vec![],
        vec!["".into()],
        vec!["!!!".into()],
        vec!["OmaScribe".into(), "offLINE".into(), "OmaScribe".into()],
        vec!["张三 预算".into(), "3000 Friday".into()],
        vec!["Éé Ω 中文 abc123".into()],
        vec!["missing".into()],
    ];
    for end in 0..=texts.len() {
        for query in &queries {
            assert_eq!(
                ranked_text(&texts[..end], query),
                legacy(&texts[..end], query)
            );
        }
    }
    let mut seed = 7_u64;
    for trial in 0..120 {
        let mut all = Vec::new();
        for _ in 0..1 + trial % 17 {
            let mut text = String::new();
            for _ in 0..1 + trial % 23 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                text.push_str(texts[(seed >> 32) as usize % texts.len()]);
                text.push(' ');
            }
            all.push(text);
        }
        let refs: Vec<_> = all.iter().map(String::as_str).collect();
        for query in &queries {
            assert_eq!(
                ranked_text(&refs, query),
                legacy(&refs, query),
                "trial {trial}"
            );
        }
    }
}
#[test]
fn append_edit_remove_reorder_and_scope_changes_never_reuse_stale_words() {
    let cache = Cache::with_budget(CACHE_BYTES);
    let query = vec!["预算".into(), "Friday OmaScribe".into()];
    let mut all = vec![
        segment(1, "预算 3000 未批准。"),
        segment(2, "OmaScribe on Friday"),
    ];
    let check = |all: &[Segment]| assert_eq!(cache.ranked("m", 1, all, &query), plain(all, &query));
    check(&all);
    assert_eq!(cache.builds.load(Ordering::Relaxed), 2);
    check(&all);
    assert_eq!(cache.builds.load(Ordering::Relaxed), 2);
    all.push(segment(3, "新预算尚未确定。"));
    check(&all);
    assert_eq!(cache.builds.load(Ordering::Relaxed), 3);
    all[0].text = "Thursday only".into(); // Even an unchanged revision must miss.
    check(&all);
    assert_eq!(cache.builds.load(Ordering::Relaxed), 4);
    all[1].revision += 1;
    all[1].speaker = Some("manual".into());
    all[1].speaker_manual = true;
    all.reverse();
    check(&all);
    assert_eq!(cache.builds.load(Ordering::Relaxed), 5);
    all.pop();
    check(&all);
    assert_eq!(cache.state.lock().unwrap().documents.len(), 2);
    {
        let state = cache.state.lock().unwrap();
        assert_eq!(
            state.bytes,
            state
                .documents
                .values()
                .map(|d| d.accounted_bytes)
                .sum::<usize>()
        );
        assert!(state.bytes <= CACHE_BYTES);
    }
    let mut other = vec![segment(1, "No budget; Friday")];
    other[0].meeting_id = "other".into();
    assert_eq!(
        cache.ranked("other", 1, &other, &query),
        plain(&other, &query)
    );
    assert_eq!(cache.state.lock().unwrap().documents.len(), 1);
    other[0].run = 2;
    other[0].text = "Withdrawn".into();
    assert_eq!(
        cache.ranked("other", 2, &other, &query),
        plain(&other, &query)
    );
    cache.clear_meeting("m");
    assert_eq!(cache.state.lock().unwrap().documents.len(), 1);
    cache.clear_meeting("other");
    assert_eq!(cache.state.lock().unwrap().bytes, 0);
    assert!(cache.state.lock().unwrap().scope.is_none());
}
#[test]
fn budget_overflow_releases_entries_and_does_not_rebuild_every_query() {
    let cache = Cache::with_budget(2000);
    let all = vec![
        segment(1, "预算 Friday"),
        segment(2, &"不同词 vocabulary 123456 ".repeat(30)),
    ];
    let query = vec!["预算 Friday".into()];
    assert_eq!(cache.ranked("m", 1, &all, &query), plain(&all, &query));
    assert!(cache.state.lock().unwrap().documents.is_empty());
    assert_eq!(cache.state.lock().unwrap().bytes, 0);
    let builds = cache.builds.load(Ordering::Relaxed);
    assert_eq!(cache.ranked("m", 1, &all, &query), plain(&all, &query));
    assert_eq!(cache.builds.load(Ordering::Relaxed), builds);
    let small = &all[..1];
    assert_eq!(cache.ranked("m", 1, small, &query), plain(small, &query));
    assert_eq!(cache.state.lock().unwrap().documents.len(), 1);
    let duplicate = vec![all[0].clone(), all[0].clone()];
    assert_eq!(
        cache.ranked("m", 1, &duplicate, &query),
        plain(&duplicate, &query)
    );
    assert!(cache.state.lock().unwrap().documents.is_empty());
    let wrong_scope = vec![segment(3, "secret")];
    assert_eq!(
        cache.ranked("other", 1, &wrong_scope, &query),
        plain(&wrong_scope, &query)
    );
    assert!(cache.state.lock().unwrap().documents.is_empty());
}
#[test]
fn deleting_meeting_drops_its_resident_index_only_after_transaction_succeeds() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let mut engine = crate::Engine::open(root.path().into())?;
    std::sync::Arc::get_mut(&mut engine)
        .unwrap()
        .retrieval_cache = Cache::with_budget(CACHE_BYTES);
    let meeting = engine.create_meeting("fictional")?;
    let mut source = segment(1, "Budget not approved");
    source.meeting_id = meeting.id.clone();
    engine.store.lock().unwrap().save_segment(&source)?;
    engine
        .retrieval_cache
        .ranked(&meeting.id, 1, &[source], &["budget".into()]);
    engine.store.lock().unwrap().conn.execute_batch("CREATE TRIGGER reject_delete BEFORE INSERT ON deleted_meetings BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;
    assert!(engine.set_meeting_deleted(&meeting.id, true).is_err());
    assert_eq!(
        engine.retrieval_cache.state.lock().unwrap().documents.len(),
        1
    );
    engine
        .store
        .lock()
        .unwrap()
        .conn
        .execute_batch("DROP TRIGGER reject_delete;")?;
    engine.set_meeting_deleted(&meeting.id, true)?;
    assert!(engine
        .retrieval_cache
        .state
        .lock()
        .unwrap()
        .documents
        .is_empty());
    engine.set_meeting_deleted(&meeting.id, false)?;
    assert!(engine
        .retrieval_cache
        .state
        .lock()
        .unwrap()
        .documents
        .is_empty());
    Ok(())
}

#[test]
fn cached_retrieval_preserves_pending_prefix_and_old_section_source_closure() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let engine = crate::Engine::open(root.path().into())?;
    let meeting = engine.create_meeting("fictional source closure")?;
    let mut snapshot = engine.snapshot(&meeting.id)?;
    snapshot.meeting.notes_cursor = 15;
    snapshot.segments = (1..=25)
        .map(|seq| {
            let mut s = segment(
                seq,
                if seq == 1 {
                    "Earlier unrelated evidence"
                } else {
                    "预算尚未批准，周五复核。"
                },
            );
            s.meeting_id = meeting.id.clone();
            s
        })
        .collect();
    let source = &snapshot.segments[0];
    snapshot.suggestions.push(crate::Suggestion {
        focus: "budget".into(),
        id: "paragraph".into(),
        meeting_id: meeting.id.clone(),
        kind: "document".into(),
        text: "Human-protected paragraph".into(),
        sources: vec![crate::Source {
            segment_id: source.id.clone(),
            revision: 1,
            start_ms: source.start_ms,
            end_ms: source.end_ms,
            quote: source.text.clone(),
            speaker_name: None,
        }],
        status: "pending".into(),
        stale: true,
    });
    let cache = Cache::with_budget(CACHE_BYTES);
    for _ in 0..2 {
        let before = serde_json::to_value(&snapshot)?;
        let plain = super::super::retrieval::retrieve(&snapshot, None);
        let indexed = super::super::retrieval::retrieve(&snapshot, Some(&cache));
        assert_eq!(indexed.cursor, 20);
        assert_eq!(indexed.cursor, plain.cursor);
        assert_eq!(indexed.keywords, plain.keywords);
        assert_eq!(
            serde_json::to_value(&indexed.segments)?,
            serde_json::to_value(&plain.segments)?
        );
        assert!((16..=20).all(|seq| indexed.segments.iter().any(|s| s.seq == seq)));
        assert!(indexed.segments.iter().any(|s| s.seq == 1));
        assert_eq!(serde_json::to_value(&snapshot)?, before);
        snapshot.segments[0].revision += 1;
        snapshot.segments[0].text = "Earlier amount withdrawn, not approved".into();
    }
    Ok(())
}

#[test]
#[ignore = "explicit isolated lexical benchmark; no model/ASR quality claim"]
fn lexical_benchmark_child() -> anyhow::Result<()> {
    use sha2::{Digest, Sha256};
    use std::time::Instant;
    let mode = std::env::var("OMASCRIBE_RETRIEVAL_BENCH_MODE")?;
    let count: usize = std::env::var("OMASCRIBE_RETRIEVAL_BENCH_COUNT")?.parse()?;
    let output = std::env::var("OMASCRIBE_RETRIEVAL_BENCH_OUTPUT")?;
    anyhow::ensure!(
        (1..=100_000).contains(&count)
            && ["legacy", "transient", "incremental"].contains(&mode.as_str())
    );
    let topics = [
        "预算 3000 尚未批准，周五继续复核。",
        "OmaScribe transcription remains offline; Lin reviews on Friday.",
        "The owner has not approved shipping; wait for Thursday.",
        "林悦负责复核，旧结论已撤回，负责人尚未确认。",
        "Speaker assignment and bilingual captions need testing.",
        "离线转写和会议纪要还需测试，不能视为已经完成。",
    ];
    let mut all: Vec<_> = (1..=count as u32)
        .map(|i| segment(i, &format!("{} item{i}", topics[i as usize % topics.len()])))
        .collect();
    let query = vec![
        "OmaScribe".into(),
        "预算 3000".into(),
        "林悦 Friday 未批准".into(),
        "Speaker assignment and bilingual captions need testing.".into(),
        "The owner has not approved shipping; wait for Thursday.".into(),
    ];
    let cache = Cache::with_budget(CACHE_BYTES);
    let mut timings = Vec::new();
    let mut inputs = Sha256::new();
    let mut rankings = Sha256::new();
    for step in 0..9 {
        if step > 0 {
            all.push(segment(
                count as u32 + step,
                &format!("{} update{step}", topics[step as usize % topics.len()]),
            ));
        }
        if step == 4 {
            all[0].text = "预算 3000 已撤回；Thursday 仍未批准。".into();
            all[0].revision += 1;
        }
        if step == 6 && all.len() > 2 {
            all.remove(1);
        }
        let before = serde_json::to_vec(&all)?;
        inputs.update(&before);
        inputs.update(serde_json::to_vec(&query)?);
        let refs: Vec<_> = all.iter().map(|s| s.text.as_str()).collect();
        let started = Instant::now();
        let result = match mode.as_str() {
            "legacy" => legacy(&refs, &query),
            "transient" => ranked_text(&refs, &query),
            _ => cache.ranked("m", 1, &all, &query),
        };
        timings.push(started.elapsed().as_secs_f64() * 1000.);
        rankings.update(serde_json::to_vec(&std::hint::black_box(result))?);
        anyhow::ensure!(
            before == serde_json::to_vec(&all)?,
            "Ranking modified source content"
        );
    }
    let state = cache.state.lock().unwrap();
    let report = serde_json::json!({"schema":1,"kind":"lexical-only","mode":mode,"initial_documents":count,
        "build":crate::build_identity(),"rank_ms":timings,"inputs_sha256":format!("{:x}",inputs.finalize()),
        "rankings_sha256":format!("{:x}",rankings.finalize()),"source_unchanged":true,
        "indexed_documents_built":cache.builds.load(Ordering::Relaxed),"retained_documents":state.documents.len(),
        "accounted_retained_bytes":state.bytes,"budget_bytes":CACHE_BYTES,"budget_fallback":state.skipped_at.is_some(),
        "limitation":"Fictional lexical ranking, no model or audio. Legacy uses sorted term summation for reproducibility; formulas/tokenization/phrase boosts/ties are unchanged. Whole-child resource measurement includes fixture generation and validation."});
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    serde_json::to_writer_pretty(file, &report)?;
    Ok(())
}
