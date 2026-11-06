//! Query-local counts and an optional bounded, single-meeting token index.
use crate::Segment;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

const CACHE_BYTES: usize = 8 * 1024 * 1024;
const CACHE_DOCUMENTS: usize = 8192;

#[cfg(test)]
#[path = "lexical_tests.rs"]
mod tests;

fn tokens(text: &str) -> Vec<String> {
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

struct Query {
    terms: Vec<String>,
    positions: HashMap<String, usize>,
    phrases: Vec<String>,
}
impl Query {
    fn new(query: &[String]) -> Self {
        let mut terms: Vec<_> = query.iter().flat_map(|q| tokens(q)).collect();
        // A stable summation order makes equal-score ordering reproducible.
        terms.sort_unstable();
        terms.dedup();
        let positions = terms
            .iter()
            .enumerate()
            .map(|(i, term)| (term.clone(), i))
            .collect();
        Self {
            terms,
            positions,
            phrases: query.iter().map(|q| q.to_lowercase()).collect(),
        }
    }
    fn phrases(&self, lower: &str) -> usize {
        self.phrases
            .iter()
            .filter(|q| lower.contains(q.as_str()))
            .count()
    }
    fn scan(&self, text: &str) -> Counts {
        let words = tokens(text);
        let mut frequencies = vec![0; self.terms.len()];
        for word in &words {
            if let Some(&position) = self.positions.get(word) {
                frequencies[position] += 1;
            }
        }
        Counts {
            length: words.len(),
            frequencies: frequencies
                .into_iter()
                .enumerate()
                .filter(|(_, n)| *n > 0)
                .collect(),
            phrases: self.phrases(&text.to_lowercase()),
        }
    }
}
struct Counts {
    length: usize,
    frequencies: Vec<(usize, usize)>,
    phrases: usize,
}
fn rank(documents: &[Counts], terms: usize) -> Vec<usize> {
    let average =
        documents.iter().map(|d| d.length).sum::<usize>() as f64 / documents.len().max(1) as f64;
    let mut df = vec![0; terms];
    for document in documents {
        for &(term, _) in &document.frequencies {
            df[term] += 1;
        }
    }
    let idf: Vec<_> = df
        .into_iter()
        .map(|hits| (1. + (documents.len() as f64 - hits as f64 + 0.5) / (hits as f64 + 0.5)).ln())
        .collect();
    let mut scores: Vec<_> = documents
        .iter()
        .enumerate()
        .map(|(i, document)| {
            let norm = 1.2 * (0.25 + 0.75 * document.length as f64 / average.max(1.));
            let lexical: f64 = document
                .frequencies
                .iter()
                .map(|&(term, count)| {
                    let frequency = count as f64;
                    idf[term] * frequency * 2.2 / (frequency + norm)
                })
                .sum();
            (i, lexical + document.phrases as f64 * 3.)
        })
        .filter(|(_, score)| *score > 0.)
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
    scores.into_iter().map(|(i, _)| i).collect()
}

/// Retain only query hits during a one-shot ranking; no corpus-wide vocabulary.
pub fn ranked_text(all: &[&str], query: &[String]) -> Vec<usize> {
    let query = Query::new(query);
    let documents: Vec<_> = all.iter().map(|s| query.scan(s)).collect();
    rank(&documents, query.terms.len())
}

struct Document {
    revision: u32,
    source: String,
    lower: String,
    frequencies: HashMap<String, usize>,
    length: usize,
    accounted_bytes: usize,
}
impl Document {
    fn new(source: &Segment) -> Self {
        let words = tokens(&source.text);
        let length = words.len();
        let mut frequencies = HashMap::new();
        for word in words {
            *frequencies.entry(word).or_default() += 1;
        }
        let source_text = source.text.clone();
        let lower = source.text.to_lowercase();
        // Conservative retained allocation accounting, not a process RSS promise.
        // Include map spare buckets/control bytes and allocator/string overhead.
        let accounted_bytes = std::mem::size_of::<Self>()
            + 128
            + source.id.len() * 2
            + source_text.capacity()
            + lower.capacity()
            + frequencies.capacity() * (std::mem::size_of::<(String, usize)>() + 32)
            + frequencies.keys().map(|s| s.capacity() + 32).sum::<usize>();
        Self {
            revision: source.revision,
            source: source_text,
            lower,
            frequencies,
            length,
            accounted_bytes,
        }
    }
    fn counts(&self, query: &Query) -> Counts {
        Counts {
            length: self.length,
            frequencies: query
                .terms
                .iter()
                .enumerate()
                .filter_map(|(i, term)| self.frequencies.get(term).map(|&count| (i, count)))
                .collect(),
            phrases: query.phrases(&self.lower),
        }
    }
}
#[derive(Default)]
struct State {
    scope: Option<(String, u32)>,
    documents: HashMap<String, Document>,
    bytes: usize,
    skipped_at: Option<(usize, usize)>,
}
pub(crate) struct Cache {
    enabled: bool,
    budget: usize,
    state: Mutex<State>,
    #[cfg(test)]
    builds: std::sync::atomic::AtomicUsize,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            enabled: std::env::var("OMASCRIBE_RETRIEVAL_CACHE").as_deref() == Ok("1"),
            budget: CACHE_BYTES,
            state: Mutex::new(State::default()),
            #[cfg(test)]
            builds: Default::default(),
        }
    }
}
impl Cache {
    #[cfg(test)]
    pub(crate) fn with_budget(budget: usize) -> Self {
        Self {
            enabled: true,
            budget,
            state: Mutex::new(State::default()),
            builds: Default::default(),
        }
    }
    pub(crate) fn clear_meeting(&self, meeting: &str) {
        let mut state = self.state.lock().unwrap();
        if state.scope.as_ref().is_some_and(|s| s.0 == meeting) {
            *state = State::default();
        }
    }
    pub(crate) fn ranked(
        &self,
        meeting: &str,
        run: u32,
        all: &[Segment],
        query: &[String],
    ) -> Vec<usize> {
        let transient = || {
            ranked_text(
                &all.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
                query,
            )
        };
        if !self.enabled {
            return transient();
        }
        let mut state = self.state.lock().unwrap();
        if state
            .scope
            .as_ref()
            .is_none_or(|s| s.0 != meeting || s.1 != run)
        {
            *state = State::default();
            state.scope = Some((meeting.to_owned(), run));
        }
        let source_bytes = all.iter().map(|s| s.text.len()).sum::<usize>();
        if state
            .skipped_at
            .is_some_and(|(bytes, count)| source_bytes >= bytes / 2 && all.len() >= count / 2)
        {
            drop(state);
            return transient();
        }
        state.skipped_at = None;
        // Refuse cross-scope/duplicate IDs rather than sharing ambiguous entries.
        let ids: HashSet<_> = all.iter().map(|s| s.id.as_str()).collect();
        if all.len() > CACHE_DOCUMENTS
            || ids.len() != all.len()
            || all.iter().any(|s| s.meeting_id != meeting || s.run != run)
            || source_bytes > self.budget / 2
        {
            *state = State {
                scope: Some((meeting.to_owned(), run)),
                skipped_at: Some((source_bytes, all.len())),
                ..State::default()
            };
            drop(state);
            return transient();
        }
        let mut removed_bytes = 0;
        state.documents.retain(|id, document| {
            let keep = ids.contains(id.as_str());
            if !keep {
                removed_bytes += document.accounted_bytes;
            }
            keep
        });
        state.bytes -= removed_bytes;
        // A source edit, even with an incorrectly unchanged revision, cannot hit.
        for source in all {
            if state
                .documents
                .get(&source.id)
                .is_some_and(|d| d.revision == source.revision && d.source == source.text)
            {
                continue;
            }
            if let Some(old) = state.documents.remove(&source.id) {
                state.bytes -= old.accounted_bytes;
            }
            let document = Document::new(source);
            #[cfg(test)]
            self.builds
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if state.bytes + document.accounted_bytes > self.budget {
                *state = State {
                    scope: Some((meeting.to_owned(), run)),
                    skipped_at: Some((source_bytes, all.len())),
                    ..State::default()
                };
                drop(state);
                return transient();
            }
            state.bytes += document.accounted_bytes;
            state.documents.insert(source.id.clone(), document);
        }
        state.documents.shrink_to_fit();
        let query = Query::new(query);
        let counts: Vec<_> = all
            .iter()
            .map(|s| state.documents[&s.id].counts(&query))
            .collect();
        rank(&counts, query.terms.len())
    }
}
