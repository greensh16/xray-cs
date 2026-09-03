//! Throughput benchmarks for xray — tracks lint latency per file and
//! lines-of-code per second across the full rule pipeline.
//!
//! Run with:
//!   cargo bench
//!   cargo bench -- --save-baseline main   # save a named baseline
//!   cargo bench -- --baseline main        # compare against saved baseline

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use xray::{
    config::Config,
    parser::{self, ParsedFile},
    rules::{self, RuleSet},
};

#[path = "corpus.rs"]
mod corpus;

/// Lines in the synthetic corpus the per-rule and pipeline benchmarks use.
/// The roadmap's figure; large enough that per-file constant costs wash out.
const CORPUS_LINES: usize = 50_000;

// ── per-file benchmarks ───────────────────────────────────────────────────────

/// Benchmark the full lint pipeline (parse + all rules) for each fixture.
fn bench_lint_fixture(c: &mut Criterion) {
    let config = Config::default();

    let fixtures: &[(&str, &str)] = &[
        ("xarray_bad", "tests/fixtures/xarray_bad.py"),
        ("dask_bad", "tests/fixtures/dask_bad.py"),
        ("numpy_bad", "tests/fixtures/numpy_bad.py"),
        ("io_bad", "tests/fixtures/io_bad.py"),
        ("clean", "tests/fixtures/clean.py"),
    ];

    let mut group = c.benchmark_group("lint_fixture");

    for (label, path) in fixtures {
        // Measure throughput in lines of code so that Criterion shows LOC/sec
        let source =
            std::fs::read_to_string(path).unwrap_or_else(|_| panic!("fixture not found: {path}"));
        let loc = source.lines().count() as u64;
        group.throughput(Throughput::Elements(loc));

        group.bench_with_input(BenchmarkId::new("file", label), path, |b, p| {
            b.iter(|| {
                let parsed = parser::parse_file(p).expect("fixture should parse");
                rules::run_all(&parsed, p, &config)
            });
        });
    }

    group.finish();
}

// ── parse-only benchmark ──────────────────────────────────────────────────────

/// Isolate the tree-sitter parse cost from the rule-check cost.
fn bench_parse_only(c: &mut Criterion) {
    let fixtures: &[(&str, &str)] = &[
        ("xarray_bad", "tests/fixtures/xarray_bad.py"),
        ("dask_bad", "tests/fixtures/dask_bad.py"),
        ("numpy_bad", "tests/fixtures/numpy_bad.py"),
        ("io_bad", "tests/fixtures/io_bad.py"),
        ("clean", "tests/fixtures/clean.py"),
    ];

    let mut group = c.benchmark_group("parse_only");

    for (label, path) in fixtures {
        let source =
            std::fs::read_to_string(path).unwrap_or_else(|_| panic!("fixture not found: {path}"));
        let loc = source.lines().count() as u64;
        group.throughput(Throughput::Elements(loc));

        group.bench_with_input(BenchmarkId::new("file", label), path, |b, p| {
            b.iter(|| parser::parse_file(p).expect("fixture should parse"));
        });
    }

    group.finish();
}

// ── aggregate benchmark ───────────────────────────────────────────────────────

/// Simulate linting an entire "project" (all fixtures in sequence) to measure
/// wall-clock cost of a typical `xray` invocation.
fn bench_all_fixtures(c: &mut Criterion) {
    let config = Config::default();
    let paths = [
        "tests/fixtures/xarray_bad.py",
        "tests/fixtures/dask_bad.py",
        "tests/fixtures/numpy_bad.py",
        "tests/fixtures/io_bad.py",
        "tests/fixtures/clean.py",
    ];

    let total_loc: u64 = paths
        .iter()
        .map(|p| {
            std::fs::read_to_string(p)
                .map(|s| s.lines().count() as u64)
                .unwrap_or(0)
        })
        .sum();

    let mut group = c.benchmark_group("lint_all_fixtures");
    group.throughput(Throughput::Elements(total_loc));

    group.bench_function("all_files", |b| {
        b.iter(|| {
            paths.iter().for_each(|p| {
                let parsed = parser::parse_file(p).expect("fixture should parse");
                let _ = rules::run_all(&parsed, p, &config);
            });
        });
    });

    group.finish();
}

// ── per-rule-domain benchmarks ────────────────────────────────────────────────

/// Throughput of each rule domain in isolation, over a 50 k-line corpus.
///
/// Parsing happens once, outside the timed section, so these measure the query
/// walk and diagnostic construction only — which is the part that grows as
/// rules are added, and the part a new rule can accidentally make quadratic.
///
/// Each domain is run unconditionally here, bypassing the import gate: the
/// synthetic corpus imports everything, and the point is to price the rule
/// pass, not the gate.
fn bench_rule_domains(c: &mut Criterion) {
    let config = Config::default();
    let sources = corpus::corpus(corpus::modules_for_lines(CORPUS_LINES));
    let loc = corpus::total_lines(&sources);

    let parsed: Vec<(String, ParsedFile)> = sources
        .into_iter()
        .filter_map(|(name, src)| parser::parse_source(src).ok().map(|p| (name, p)))
        .collect();

    let mut group = c.benchmark_group("rule_domain");
    group.throughput(Throughput::Elements(loc));

    macro_rules! bench_domain {
        ($label:expr, $ty:ty) => {
            group.bench_function($label, |b| {
                b.iter(|| {
                    let mut n = 0usize;
                    for (name, file) in &parsed {
                        n += <$ty>::check(file, name, &config).len();
                    }
                    n
                });
            });
        };
    }

    bench_domain!("xarray", rules::xarray::XarrayRules);
    bench_domain!("dask", rules::dask::DaskRules);
    bench_domain!("numpy", rules::numpy::NumpyRules);
    bench_domain!("pandas", rules::pandas::PandasRules);
    bench_domain!("scipy", rules::scipy::ScipyRules);
    bench_domain!("io", rules::io::IoRules);

    // Everything together, through the real import gate and the suppression,
    // redundancy and sort passes that `run_all` adds on top.
    group.bench_function("all_via_run_all", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for (name, file) in &parsed {
                n += rules::run_all(file, name, &config).len();
            }
            n
        });
    });

    group.finish();
}

// ── whole-corpus pipeline ─────────────────────────────────────────────────────

/// Parse + check a 50 k-line corpus, i.e. what an uncached `xray` run costs
/// per line. The headline number.
fn bench_corpus_pipeline(c: &mut Criterion) {
    let config = Config::default();
    let sources = corpus::corpus(corpus::modules_for_lines(CORPUS_LINES));
    let loc = corpus::total_lines(&sources);

    let mut group = c.benchmark_group("corpus");
    group.throughput(Throughput::Elements(loc));
    group.sample_size(20);

    group.bench_function("parse_and_check_50k", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for (name, src) in &sources {
                if let Ok(file) = parser::parse_source(src.clone()) {
                    n += rules::run_all(&file, name, &config).len();
                }
            }
            n
        });
    });

    group.bench_function("parse_only_50k", |b| {
        b.iter(|| {
            for (_, src) in &sources {
                let _ = parser::parse_source(src.clone());
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_lint_fixture,
    bench_parse_only,
    bench_all_fixtures,
    bench_rule_domains,
    bench_corpus_pipeline
);
criterion_main!(benches);
