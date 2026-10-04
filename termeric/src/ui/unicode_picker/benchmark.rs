//! The Unicode picker's benchmark workload, run by the release benchmark
//! lane (`test_support`) and, for its correctness checks, by the tests.

use super::{
    UNICODE_RESULT_MEMORY_BUDGET, UNICODE_SEARCH_RESULT_LIMIT, annotations,
    ranked_results_owned_bytes, search_unicode_characters, unicode_name_index,
};

pub(crate) struct UnicodePickerBenchmark {
    pub annotation_validation_ms: Vec<f64>,
    pub keyword_query_ms: Vec<f64>,
    pub exhaustive_batch_ms: Vec<f64>,
    pub annotation_static_bytes: Vec<f64>,
    pub result_owned_bytes: Vec<f64>,
}

pub(crate) fn benchmark_unicode_picker() -> UnicodePickerBenchmark {
    let mut annotation_validation_ms = Vec::with_capacity(5);
    let mut annotation_static_bytes = Vec::with_capacity(5);
    for _ in 0..5 {
        let started = std::time::Instant::now();
        let index = annotations::validate_embedded_index();
        std::hint::black_box(index.annotation(index.len() - 1));
        annotation_validation_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        annotation_static_bytes.push(index.static_bytes() as f64);
    }

    unicode_name_index();
    annotations::index();
    // Keep wall-clock enforcement in the serial release benchmark lane, not
    // the concurrently scheduled correctness suite. Every sample performs the
    // same five-name-query workload and deterministic result checks.
    let mut exhaustive_batch_ms = Vec::with_capacity(5);
    for _ in 0..5 {
        let started = std::time::Instant::now();
        check_exhaustive_query_batch();
        exhaustive_batch_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    let mut keyword_query_ms = Vec::with_capacity(5);
    let mut result_owned_bytes = Vec::with_capacity(5);
    for (query, expected) in [
        ("heart blue", "💙"),
        ("party pinata", "🪅"),
        ("thumb medium skin tone", "👍🏽"),
        ("flag united states", "🇺🇸"),
        ("woman judge medium dark skin tone", "👩🏾‍⚖️"),
    ] {
        let started = std::time::Instant::now();
        let results = search_unicode_characters(query);
        keyword_query_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        assert!(
            results
                .iter()
                .any(|result| result.selection.text == expected),
            "benchmark query {query:?} missed {expected:?}"
        );
        result_owned_bytes.push(ranked_results_owned_bytes(&results) as f64);
        std::hint::black_box(results);
    }

    UnicodePickerBenchmark {
        annotation_validation_ms,
        keyword_query_ms,
        exhaustive_batch_ms,
        annotation_static_bytes,
        result_owned_bytes,
    }
}

pub(super) fn check_exhaustive_query_batch() {
    for query in [
        "letter",
        "black",
        "arabic letter",
        "mathematical symbol",
        "cjk unified",
    ] {
        let results = search_unicode_characters(query);
        assert!(!results.is_empty(), "{query}");
        assert!(results.len() <= UNICODE_SEARCH_RESULT_LIMIT, "{query}");
        assert!(
            ranked_results_owned_bytes(&results) <= UNICODE_RESULT_MEMORY_BUDGET,
            "{query}"
        );
        if query == "letter" {
            assert_eq!(results.len(), UNICODE_SEARCH_RESULT_LIMIT);
        }
        std::hint::black_box(results);
    }
}
