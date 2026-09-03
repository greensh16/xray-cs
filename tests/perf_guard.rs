//! Performance regression guard.
//!
//! Run explicitly, in release:
//!
//! ```bash
//! cargo test --release --test perf_guard -- --ignored --nocapture
//! ```
//!
//! ## Why a floor and not a baseline diff
//!
//! The obvious design is to compare against a saved criterion baseline and
//! fail on any slowdown. That does not survive CI. Shared runners vary by 2–3×
//! between jobs depending on the neighbours they land next to, so a percentage
//! gate produces a stream of false failures, and a team that learns to ignore
//! a red check has a worse gate than no gate.
//!
//! These tests assert a **generous absolute floor** instead — roughly an order
//! of magnitude slower than the machine this was written on. That will not
//! notice a 20 % regression, and is not meant to: `cargo bench` is the tool for
//! that, run deliberately on a quiet machine. What this catches is the failure
//! that actually ships — a rule whose cost is quadratic in file size, or a
//! query that rebuilds something per match. Those are 10–1000×, not 20 %.
//!
//! Every measurement here is **single-threaded and sequential** on purpose.
//! The real CLI uses rayon, so its wall time depends on the runner's core
//! count; timing the sequential path keeps the number comparable across
//! machines.

use std::time::{Duration, Instant};

use xray::{config::Config, parser, rules};

#[path = "../benches/corpus.rs"]
mod corpus;

/// Lines in the synthetic corpus, matching the benchmark suite.
const CORPUS_LINES: usize = 50_000;

/// Sequential parse + check of a 50 k-line corpus.
///
/// Measured at ~0.8 s on an M-series laptop, release build. The gate sits at
/// 8 s: ~10× headroom for a loaded CI runner, while a quadratic rule would
/// blow through it by orders of magnitude.
const CORPUS_BUDGET: Duration = Duration::from_secs(8);

/// Parsing alone, same corpus. Measured at ~0.24 s; gate at 4 s.
const PARSE_BUDGET: Duration = Duration::from_secs(4);

fn report(label: &str, elapsed: Duration, lines: u64, budget: Duration) {
    let per_sec = lines as f64 / elapsed.as_secs_f64();
    println!(
        "  {label:<22} {:>7.0} ms  {:>9.0} lines/s  (budget {:.0} s)",
        elapsed.as_secs_f64() * 1000.0,
        per_sec,
        budget.as_secs_f64()
    );
}

#[test]
#[ignore = "performance guard: run with --release --ignored"]
fn full_pipeline_stays_within_budget() {
    let config = Config::default();
    let sources = corpus::corpus(corpus::modules_for_lines(CORPUS_LINES));
    let lines = corpus::total_lines(&sources);
    assert!(
        lines >= CORPUS_LINES as u64,
        "corpus should be at least {CORPUS_LINES} lines, got {lines}"
    );

    let start = Instant::now();
    let mut findings = 0usize;
    for (name, src) in &sources {
        let parsed = parser::parse_source(src.clone()).expect("synthetic corpus must parse");
        findings += rules::run_all(&parsed, name, &config).len();
    }
    let elapsed = start.elapsed();
    report("parse + all rules", elapsed, lines, CORPUS_BUDGET);

    // A corpus that produces nothing would make the timing meaningless — it
    // would mean the rules never reached their diagnostic paths.
    assert!(
        findings > 0,
        "synthetic corpus should trigger rules; got {findings} findings"
    );
    assert!(
        elapsed < CORPUS_BUDGET,
        "linting {lines} lines took {elapsed:?}, over the {CORPUS_BUDGET:?} budget — \
         this is an order-of-magnitude regression, not noise"
    );
}

#[test]
#[ignore = "performance guard: run with --release --ignored"]
fn parsing_stays_within_budget() {
    let sources = corpus::corpus(corpus::modules_for_lines(CORPUS_LINES));
    let lines = corpus::total_lines(&sources);

    let start = Instant::now();
    for (_, src) in &sources {
        let _ = parser::parse_source(src.clone()).expect("synthetic corpus must parse");
    }
    let elapsed = start.elapsed();
    report("parse only", elapsed, lines, PARSE_BUDGET);

    assert!(
        elapsed < PARSE_BUDGET,
        "parsing {lines} lines took {elapsed:?}, over the {PARSE_BUDGET:?} budget"
    );
}

/// Cost must grow linearly in corpus size, not quadratically.
///
/// This is the check a wall-clock floor cannot make on its own: a rule that is
/// O(n²) in file *count* or file *size* can still fit inside a generous budget
/// at 50 k lines and fall over on a real repository. Comparing two sizes
/// catches the shape of the curve rather than a point on it.
#[test]
#[ignore = "performance guard: run with --release --ignored"]
fn cost_is_linear_in_corpus_size() {
    let config = Config::default();

    let time_for = |n: usize| {
        let sources = corpus::corpus(n);
        let start = Instant::now();
        for (name, src) in &sources {
            let parsed = parser::parse_source(src.clone()).expect("must parse");
            let _ = rules::run_all(&parsed, name, &config);
        }
        start.elapsed().as_secs_f64()
    };

    let small = corpus::modules_for_lines(CORPUS_LINES / 4);
    let large = small * 4;

    // Warm up so the first measurement does not pay one-off costs — the
    // tree-sitter queries compile lazily on first use, once per process.
    let _ = time_for(4);

    let t_small = time_for(small);
    let t_large = time_for(large);
    let ratio = t_large / t_small;
    println!(
        "  scaling               {small} → {large} modules: {t_small:.3}s → {t_large:.3}s  (×{ratio:.2}, linear = ×4)"
    );

    // Linear would be 4×. Allow up to 8× for measurement noise and cache
    // effects at the larger size; quadratic would be 16×.
    assert!(
        ratio < 8.0,
        "4× the corpus took {ratio:.1}× the time — cost is growing faster than \
         linearly, which is the signature of a quadratic rule"
    );
}
