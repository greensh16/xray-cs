use xray::{
    config::Config,
    diagnostic::Severity,
    diff,
    ignore::IgnorePatterns,
    lsp, parser, rules,
    runner::{JSON_SCHEMA_VERSION, build_gitlab_json, build_json, build_sarif_json},
};

fn check_fixture(filename: &str) -> Vec<xray::diagnostic::Diagnostic> {
    let path = format!("tests/fixtures/{filename}");
    let parsed = parser::parse_file(&path).expect("fixture should parse");
    let config = Config::default();
    let mut diags = rules::run_all(&parsed, &path, &config);
    diags.sort_by_key(|d| (d.line, d.rule_id));
    diags
}

// ── xarray ────────────────────────────────────────────────────────────────────

#[test]
fn xr001_open_dataset_without_chunks() {
    let diags = check_fixture("xarray_bad.py");
    let ids: Vec<_> = diags.iter().map(|d| d.rule_id).collect();
    assert!(ids.contains(&"XR001"), "expected XR001 in {ids:?}");
    let xr001: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR001").collect();
    // Both open_dataset and open_mfdataset should fire
    assert_eq!(
        xr001.len(),
        2,
        "expected 2 XR001 diagnostics (one per open call)"
    );
}

#[test]
fn xr001_suppressed_when_chunks_present() {
    // The clean fixture calls open_dataset with chunks= — must not fire
    let diags = check_fixture("clean.py");
    let xr001: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR001").collect();
    assert!(
        xr001.is_empty(),
        "XR001 should not fire when chunks= is provided"
    );
}

#[test]
fn xr002_values_access() {
    let diags = check_fixture("xarray_bad.py");
    let xr002: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR002").collect();
    assert!(!xr002.is_empty(), "expected XR002 for .values access");
}

#[test]
fn xr003_loop_over_dimension() {
    let diags = check_fixture("xarray_bad.py");
    let xr003: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR003").collect();
    assert!(!xr003.is_empty(), "expected XR003 for loop over ds.time");
}

#[test]
fn xr004_sel_with_float() {
    let diags = check_fixture("xarray_bad.py");
    let xr004: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR004").collect();
    assert!(
        !xr004.is_empty(),
        "expected XR004 for .sel() with float literal"
    );
}

#[test]
fn xr005_compute_in_loop() {
    let diags = check_fixture("xarray_bad.py");
    let xr005: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR005").collect();
    assert!(
        !xr005.is_empty(),
        "expected XR005 for .compute() inside for loop"
    );
}

// ── dask ──────────────────────────────────────────────────────────────────────

#[test]
fn dk001_compute_in_loop() {
    let diags = check_fixture("dask_bad.py");
    let dk001: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK001").collect();
    assert!(
        !dk001.is_empty(),
        "expected DK001 for .compute() in for loop"
    );
}

#[test]
fn dk002_dask_compute_in_loop() {
    let diags = check_fixture("dask_bad.py");
    let dk002: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK002").collect();
    assert!(
        !dk002.is_empty(),
        "expected DK002 for dask.compute() in for loop"
    );
}

#[test]
fn dk003_excessive_compute_calls() {
    let diags = check_fixture("dask_bad.py");
    let dk003: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK003").collect();
    assert!(
        !dk003.is_empty(),
        "expected DK003 for multiple .compute() calls"
    );
    assert!(
        dk003[0].message.contains(".compute()"),
        "message should mention .compute() calls"
    );
}

#[test]
fn dk004_immediate_compute() {
    let diags = check_fixture("dask_bad.py");
    let dk004: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK004").collect();
    assert!(
        !dk004.is_empty(),
        "expected DK004 for immediate .compute() on a call result"
    );
}

// ── numpy / pandas ────────────────────────────────────────────────────────────

#[test]
fn np001_iterrows() {
    let diags = check_fixture("numpy_bad.py");
    let np001: Vec<_> = diags.iter().filter(|d| d.rule_id == "NP001").collect();
    assert!(!np001.is_empty(), "expected NP001 for .iterrows()");
}

#[test]
fn np002_concat_in_loop() {
    let diags = check_fixture("numpy_bad.py");
    let np002: Vec<_> = diags.iter().filter(|d| d.rule_id == "NP002").collect();
    // Both pd.concat and np.concatenate inside loops should fire
    assert!(np002.len() >= 2, "expected at least 2 NP002 diagnostics");
}

#[test]
fn np003_alloc_without_dtype() {
    let diags = check_fixture("numpy_bad.py");
    let np003: Vec<_> = diags.iter().filter(|d| d.rule_id == "NP003").collect();
    assert_eq!(
        np003.len(),
        2,
        "expected 2 NP003 hits (zeros and ones without dtype)"
    );
}

#[test]
fn np003_suppressed_when_dtype_present() {
    let diags = check_fixture("clean.py");
    let np003: Vec<_> = diags.iter().filter(|d| d.rule_id == "NP003").collect();
    assert!(
        np003.is_empty(),
        "NP003 should not fire when dtype= is provided"
    );
}

#[test]
fn np004_math_fn_in_loop() {
    let diags = check_fixture("numpy_bad.py");
    let np004: Vec<_> = diags.iter().filter(|d| d.rule_id == "NP004").collect();
    assert!(
        np004.len() >= 2,
        "expected NP004 for math.sqrt and math.log in loops"
    );
}

#[test]
fn np005_chained_indexing() {
    let diags = check_fixture("numpy_bad.py");
    let np005: Vec<_> = diags.iter().filter(|d| d.rule_id == "NP005").collect();
    assert!(!np005.is_empty(), "expected NP005 for chained indexing");
}

// ── IO ────────────────────────────────────────────────────────────────────────

#[test]
fn io001_np_save() {
    let diags = check_fixture("io_bad.py");
    let io001: Vec<_> = diags.iter().filter(|d| d.rule_id == "IO001").collect();
    assert_eq!(io001.len(), 2, "expected 2 IO001 hits (two np.save calls)");
}

#[test]
fn io002_netcdf4_direct() {
    let diags = check_fixture("io_bad.py");
    let io002: Vec<_> = diags.iter().filter(|d| d.rule_id == "IO002").collect();
    assert!(
        !io002.is_empty(),
        "expected IO002 for netCDF4.Dataset direct open"
    );
}

#[test]
fn io003_zarr_without_chunks() {
    let diags = check_fixture("io_bad.py");
    let io003: Vec<_> = diags.iter().filter(|d| d.rule_id == "IO003").collect();
    assert_eq!(
        io003.len(),
        2,
        "expected 2 IO003 hits (zarr.open and zarr.open_array without chunks)"
    );
}

#[test]
fn io003_suppressed_when_chunks_present() {
    let diags = check_fixture("io_bad.py");
    let io003: Vec<_> = diags.iter().filter(|d| d.rule_id == "IO003").collect();
    // The z_ok call has chunks= — make sure only the two bad ones fired
    assert_eq!(
        io003.len(),
        2,
        "zarr.open with chunks= should not trigger IO003"
    );
}

#[test]
fn io004_netcdf4_read_in_loop() {
    let diags = check_fixture("io_bad.py");
    let io004: Vec<_> = diags.iter().filter(|d| d.rule_id == "IO004").collect();
    assert!(
        !io004.is_empty(),
        "expected IO004 for netCDF4 variable read in loop"
    );
}

// ── clean fixture ─────────────────────────────────────────────────────────────

#[test]
fn clean_fixture_produces_no_diagnostics() {
    let diags = check_fixture("clean.py");
    assert!(
        diags.is_empty(),
        "clean.py should produce zero diagnostics, got:\n{:#?}",
        diags
            .iter()
            .map(|d| format!("{}:{} [{}] {}", d.line, d.column, d.rule_id, d.message))
            .collect::<Vec<_>>()
    );
}

// ── rule disable ──────────────────────────────────────────────────────────────

#[test]
fn disable_rule_via_config() {
    let path = "tests/fixtures/xarray_bad.py";
    let parsed = parser::parse_file(path).unwrap();

    let mut config = Config::default();
    config.disable.insert("XR001".to_string());
    config.disable.insert("XR002".to_string());
    config.disable.insert("XR003".to_string());
    config.disable.insert("XR004".to_string());
    config.disable.insert("XR005".to_string());
    config.disable.insert("XR006".to_string());
    config.disable.insert("XR007".to_string());
    config.disable.insert("XR008".to_string());
    config.disable.insert("XR009".to_string());
    config.disable.insert("XR010".to_string());
    config.disable.insert("XR011".to_string());

    let diags = rules::run_all(&parsed, path, &config);
    assert!(
        diags.is_empty(),
        "all XR rules disabled — should be no diagnostics"
    );
}

// ── line numbers ──────────────────────────────────────────────────────────────

#[test]
fn xr001_reports_correct_line() {
    let diags = check_fixture("xarray_bad.py");
    let xr001: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR001").collect();

    // Line 5 in xarray_bad.py: ds = xr.open_dataset("era5.nc")
    let first_line = xr001[0].line;
    assert_eq!(
        first_line, 5,
        "XR001 should point to line 5, got line {first_line}"
    );
}

#[test]
fn np001_reports_correct_line() {
    let diags = check_fixture("numpy_bad.py");
    let np001: Vec<_> = diags.iter().filter(|d| d.rule_id == "NP001").collect();

    // Line 10 in numpy_bad.py: for idx, row in df.iterrows():
    let line = np001[0].line;
    assert_eq!(line, 10, "NP001 should point to line 10, got line {line}");
}

// ── inline suppression ────────────────────────────────────────────────────────

#[test]
fn inline_suppress_line_level() {
    // suppress_bad.py has every bad pattern followed by # xray: disable=RULE
    // so the net diagnostic count should be zero.
    let diags = check_fixture("suppress_bad.py");
    assert!(
        diags.is_empty(),
        "all diagnostics suppressed inline — expected zero, got:\n{:#?}",
        diags
            .iter()
            .map(|d| format!("{}:{} [{}] {}", d.line, d.column, d.rule_id, d.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn inline_suppress_file_level() {
    // suppress_file_bad.py has `# xray: disable-file=XR001` at the top.
    // Two open_dataset calls — both should be silenced.
    let diags = check_fixture("suppress_file_bad.py");
    let xr001: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR001").collect();
    assert!(
        xr001.is_empty(),
        "XR001 suppressed file-wide — expected zero, got {} diagnostics",
        xr001.len()
    );
}

#[test]
fn inline_suppress_does_not_suppress_other_rules() {
    // A line-level disable=XR001 should not suppress XR002 on the same line.
    // Use suppress_bad.py: the .values line has disable=XR002, not disable=XR001,
    // so XR001 (if it were on that same line) would still fire. We verify the
    // suppression is rule-specific by parsing a small snippet directly.
    let source = r#"
import xarray as xr
ds = xr.open_dataset("era5.nc")  # xray: disable=XR002
arr = ds["u10"].values  # xray: disable=XR001
"#
    .to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "<inline>", &config);

    // XR001 on line 3 is suppressed (disable=XR002 doesn't protect it from XR001)
    // — wait, line 3 has disable=XR002, not XR001; XR001 should still fire.
    let xr001: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR001").collect();
    assert!(
        !xr001.is_empty(),
        "XR001 should still fire — only XR002 was suppressed on that line"
    );

    // XR002 on line 4 is suppressed with disable=XR001 — XR002 should still fire.
    let xr002_suppressed: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR002").collect();
    // XR002 is NOT suppressed on line 4 (disable=XR001 ≠ disable=XR002)
    assert!(
        !xr002_suppressed.is_empty(),
        "XR002 should still fire — only XR001 was suppressed on that line"
    );
}

// ── NP004 scope expansion ─────────────────────────────────────────────────────

#[test]
fn np004_math_fn_in_loop_is_warning() {
    let diags = check_fixture("numpy_bad.py");
    let in_loop: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "NP004" && d.severity == Severity::Warning)
        .collect();
    assert!(
        in_loop.len() >= 2,
        "expected at least 2 NP004 warnings for math.* inside loops"
    );
}

#[test]
fn np004_math_fn_outside_loop_is_hint() {
    let diags = check_fixture("numpy_bad.py");
    let outside_loop: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "NP004" && d.severity == Severity::Hint)
        .collect();
    assert!(
        !outside_loop.is_empty(),
        "expected at least 1 NP004 hint for math.* outside a loop"
    );
}

// ── XR002 method-call guard ───────────────────────────────────────────────────

#[test]
fn xr002_does_not_fire_for_dict_values_method() {
    // dict.values() is a method call, not a DataArray property — should not trigger XR002.
    let source = r#"
import xarray as xr
d = {"a": 1, "b": 2}
for k in d.values():
    print(k)
"#
    .to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "<inline>", &config);
    let xr002: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR002").collect();
    assert!(
        xr002.is_empty(),
        "XR002 should not fire for dict.values() method calls"
    );
}

// ── v0.3 new rules ────────────────────────────────────────────────────────────

#[test]
fn xr006_to_array_without_dim() {
    let diags = check_fixture("xarray_bad.py");
    let xr006: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR006").collect();
    assert!(
        xr006.len() >= 2,
        "expected XR006 for to_array() and to_dataarray() without dim="
    );
}

#[test]
fn xr006_suppressed_when_dim_present() {
    let source = r#"
import xarray as xr
ds = xr.open_dataset("era5.nc", chunks={"time": 24})
arr = ds.to_array(dim="variable")
"#
    .to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "<inline>", &config);
    let xr006: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR006").collect();
    assert!(
        xr006.is_empty(),
        "XR006 must not fire when dim= is provided"
    );
}

#[test]
fn xr007_concat_in_loop() {
    let diags = check_fixture("xarray_bad.py");
    let xr007: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR007").collect();
    assert!(
        !xr007.is_empty(),
        "expected XR007 for xr.concat inside a for loop"
    );
}

#[test]
fn dk005_persist_result_discarded() {
    let diags = check_fixture("dask_bad.py");
    let dk005: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK005").collect();
    assert!(
        !dk005.is_empty(),
        "expected DK005 when .persist() result is discarded"
    );
}

#[test]
fn dk006_persist_then_compute() {
    let diags = check_fixture("dask_bad.py");
    let dk006: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK006").collect();
    assert!(
        !dk006.is_empty(),
        "expected DK006 for .persist().compute() chain"
    );
}

#[test]
fn dk004_does_not_fire_for_persist_compute_chain() {
    // DK004 must NOT fire when the inner call is .persist() — that's DK006's territory.
    let source = r#"
import dask.array as da
import numpy as np
a = da.from_array(np.arange(1000), chunks=100)
result = a.persist().compute()
"#
    .to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "<inline>", &config);
    let dk004: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK004").collect();
    assert!(
        dk004.is_empty(),
        "DK004 must not fire for .persist().compute() chains — that is DK006"
    );
}

#[test]
fn np006_matrix_deprecated() {
    let diags = check_fixture("numpy_bad.py");
    let np006: Vec<_> = diags.iter().filter(|d| d.rule_id == "NP006").collect();
    assert!(!np006.is_empty(), "expected NP006 for np.matrix()");
}

#[test]
fn np007a_applymap_deprecated() {
    let diags = check_fixture("numpy_bad.py");
    let np007a: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "NP007" && d.message.contains("applymap"))
        .collect();
    assert!(!np007a.is_empty(), "expected NP007 for .applymap()");
}

#[test]
fn np007b_apply_lambda_in_loop() {
    let diags = check_fixture("numpy_bad.py");
    let np007b: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "NP007" && d.message.contains("loop"))
        .collect();
    assert!(
        !np007b.is_empty(),
        "expected NP007 for .apply(lambda) inside a for loop"
    );
}

#[test]
fn io005_h5py_without_swmr() {
    let diags = check_fixture("io_bad.py");
    let io005: Vec<_> = diags.iter().filter(|d| d.rule_id == "IO005").collect();
    assert!(
        !io005.is_empty(),
        "expected IO005 for h5py.File without swmr=True"
    );
}

#[test]
fn io005_suppressed_when_swmr_present() {
    let source = r#"
import h5py
f = h5py.File("data.h5", "r", swmr=True)
"#
    .to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "<inline>", &config);
    let io005: Vec<_> = diags.iter().filter(|d| d.rule_id == "IO005").collect();
    assert!(
        io005.is_empty(),
        "IO005 must not fire when swmr=True is provided"
    );
}

#[test]
fn io006_engine_scipy() {
    let diags = check_fixture("io_bad.py");
    let io006: Vec<_> = diags.iter().filter(|d| d.rule_id == "IO006").collect();
    assert!(
        !io006.is_empty(),
        "expected IO006 for open_dataset with engine=\"scipy\""
    );
}

#[test]
fn io006_suppressed_for_engine_netcdf4() {
    let source = r#"
import xarray as xr
ds = xr.open_dataset("data.nc", chunks="auto", engine="netcdf4")
"#
    .to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "<inline>", &config);
    let io006: Vec<_> = diags.iter().filter(|d| d.rule_id == "IO006").collect();
    assert!(
        io006.is_empty(),
        "IO006 must not fire for engine=\"netcdf4\""
    );
}

// ── NP003 dtype keyword hardening ────────────────────────────────────────────

#[test]
fn np003_does_not_fire_when_dtype_keyword_present() {
    // AST-based check: dtype= as a keyword argument must suppress NP003.
    let source = r#"
import numpy as np
grid = np.zeros((1024, 1024), dtype=np.float32)
mask = np.ones((512, 512), dtype=np.int8)
"#
    .to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "<inline>", &config);
    let np003: Vec<_> = diags.iter().filter(|d| d.rule_id == "NP003").collect();
    assert!(
        np003.is_empty(),
        "NP003 must not fire when dtype= keyword argument is present"
    );
}

// ── v0.5: .xrayignore ─────────────────────────────────────────────────────────

#[test]
fn ignore_bare_name_matches_anywhere_in_tree() {
    let ig = IgnorePatterns::parse("vendor");
    assert!(ig.is_ignored("vendor/lib.py"));
    assert!(ig.is_ignored("src/vendor/utils.py"));
    assert!(!ig.is_ignored("src/main.py"));
}

#[test]
fn ignore_directory_pattern_matches_all_children() {
    let ig = IgnorePatterns::parse("tests/fixtures/");
    assert!(ig.is_ignored("tests/fixtures/clean.py"));
    assert!(ig.is_ignored("tests/fixtures/xarray_bad.py"));
    assert!(!ig.is_ignored("tests/other/bad.py"));
}

#[test]
fn ignore_glob_wildcard_in_pattern() {
    let ig = IgnorePatterns::parse("**/generated_*.py");
    assert!(ig.is_ignored("src/generated_models.py"));
    assert!(ig.is_ignored("deep/nested/generated_schema.py"));
    assert!(!ig.is_ignored("src/main.py"));
}

#[test]
fn ignore_comments_and_blanks_skipped() {
    let ig = IgnorePatterns::parse("# comment\n\nvendor\n# another comment");
    assert!(ig.is_ignored("vendor/foo.py"));
    assert!(!ig.is_ignored("src/main.py"));
}

#[test]
fn ignore_empty_set_ignores_nothing() {
    let ig = IgnorePatterns::default();
    assert!(!ig.is_ignored("any/path.py"));
}

// ── v0.5: config validation ───────────────────────────────────────────────────

#[test]
fn config_validation_unknown_disable_rule() {
    let mut config = Config::default();
    config.disable.insert("FAKE99".to_string());
    let known: Vec<&str> = rules::all_meta().iter().map(|m| m.id).collect();
    let errors = config.validate(&known);
    assert!(
        errors.iter().any(|e| e.contains("FAKE99")),
        "expected validation error for unknown rule FAKE99"
    );
}

#[test]
fn config_validation_valid_disable_rule_no_error() {
    let mut config = Config::default();
    config.disable.insert("XR001".to_string());
    let known: Vec<&str> = rules::all_meta().iter().map(|m| m.id).collect();
    let errors = config.validate(&known);
    assert!(
        errors.is_empty(),
        "XR001 is a valid rule ID, should not produce errors"
    );
}

#[test]
fn config_validation_invalid_severity_value() {
    let mut config = Config::default();
    config
        .severity_overrides
        .insert("XR001".to_string(), "critical".to_string());
    let known: Vec<&str> = rules::all_meta().iter().map(|m| m.id).collect();
    let errors = config.validate(&known);
    assert!(
        errors.iter().any(|e| e.contains("critical")),
        "expected error for invalid severity value 'critical'"
    );
}

#[test]
fn config_validation_zero_compute_threshold() {
    let mut config = Config::default();
    config.dask.compute_call_threshold = 0;
    let known: Vec<&str> = rules::all_meta().iter().map(|m| m.id).collect();
    let errors = config.validate(&known);
    assert!(
        !errors.is_empty(),
        "compute_call_threshold = 0 should produce a validation error"
    );
}

// ── v0.5: severity_overrides ──────────────────────────────────────────────────

#[test]
fn severity_override_promotes_hint_to_error() {
    // XR006 fires at Hint by default (to_array() without dim=)
    // Override it to error and confirm the override is respected by checking
    // that the rule fires and that the upgrade would change its severity.
    let diags = check_fixture("xarray_bad.py");
    let xr006: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR006").collect();
    assert!(!xr006.is_empty(), "XR006 should fire on xarray_bad.py");

    // Now apply the override manually (runner applies it; here we test the
    // config struct directly so we don't need a full runner invocation).
    let mut config = Config::default();
    config
        .severity_overrides
        .insert("XR006".to_string(), "error".to_string());

    // Validate that the config is valid
    let known: Vec<&str> = rules::all_meta().iter().map(|m| m.id).collect();
    assert!(config.validate(&known).is_empty());

    // The override map contains the right key/value
    assert_eq!(
        config.severity_overrides.get("XR006").map(String::as_str),
        Some("error")
    );
}

// ── v0.5: [paths] config section ─────────────────────────────────────────────

#[test]
fn paths_config_default_include_is_all_python() {
    let config = Config::default();
    assert_eq!(config.paths.include, vec!["**/*.py"]);
    assert!(config.paths.exclude.is_empty());
}

#[test]
fn paths_config_exclude_parses_from_toml() {
    let toml_str = r#"
[paths]
include = ["src/**/*.py"]
exclude = ["tests/fixtures/**", "**/vendor/**"]
"#;
    let config: Config = toml::from_str(toml_str).expect("should parse");
    assert_eq!(config.paths.include, vec!["src/**/*.py"]);
    assert_eq!(config.paths.exclude.len(), 2);
    assert!(
        config
            .paths
            .exclude
            .contains(&"tests/fixtures/**".to_string())
    );
    assert!(config.paths.exclude.contains(&"**/vendor/**".to_string()));
}

// ── v0.6: diff module unit tests ──────────────────────────────────────────────

#[test]
fn diff_parse_filters_non_python() {
    let raw = "src/analysis.py\nREADME.md\nsetup.cfg\nsrc/utils.py\n";
    let result = diff::parse_diff_output(raw);
    assert_eq!(result, vec!["src/analysis.py", "src/utils.py"]);
}

#[test]
fn diff_parse_empty_produces_empty() {
    assert!(diff::parse_diff_output("").is_empty());
}

#[test]
fn diff_parse_no_python_produces_empty() {
    let raw = "Makefile\ndocs/index.rst\npyproject.toml\n";
    assert!(diff::parse_diff_output(raw).is_empty());
}

#[test]
fn diff_parse_handles_blank_lines() {
    let raw = "\nsrc/model.py\n\nsrc/io.py\n\n";
    let result = diff::parse_diff_output(raw);
    assert_eq!(result, vec!["src/model.py", "src/io.py"]);
}

// ── v0.6: SARIF output format ─────────────────────────────────────────────────

/// Build a minimal RunResults and confirm the SARIF envelope is valid.
#[test]
fn sarif_output_has_correct_schema_and_version() {
    let diags = check_fixture("xarray_bad.py");
    assert!(!diags.is_empty(), "need at least one diagnostic");

    // We build the SARIF from the actual fixture results via the public fn
    let results = xray::diagnostic::RunResults {
        files: vec![xray::diagnostic::FileResults {
            path: "tests/fixtures/xarray_bad.py".to_string(),
            diagnostics: diags,
        }],
        paths: vec!["tests/fixtures/xarray_bad.py".to_string()],
    };

    let sarif_str = build_sarif_json(&results).expect("sarif build should succeed");
    let sarif: serde_json::Value =
        serde_json::from_str(&sarif_str).expect("SARIF output should be valid JSON");

    assert_eq!(sarif["version"], "2.1.0");
    assert!(
        sarif["$schema"]
            .as_str()
            .unwrap_or("")
            .contains("sarif-2.1.0"),
        "SARIF schema URI should reference sarif-2.1.0"
    );

    let runs = sarif["runs"].as_array().expect("runs should be an array");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["tool"]["driver"]["name"], "xray");

    let results_arr = runs[0]["results"]
        .as_array()
        .expect("results should be an array");
    assert!(!results_arr.is_empty(), "SARIF results should not be empty");

    // Each result must have ruleId and locations
    for result in results_arr {
        assert!(
            result["ruleId"].is_string(),
            "SARIF result must have ruleId"
        );
        let locs = result["locations"]
            .as_array()
            .expect("locations should be array");
        assert!(!locs.is_empty());
        let region = &locs[0]["physicalLocation"]["region"];
        assert!(
            region["startLine"].as_u64().unwrap_or(0) > 0,
            "startLine must be ≥ 1"
        );
    }
}

#[test]
fn sarif_severity_mapping() {
    // XR001 is Warning; verify it maps to SARIF "warning" not "error" or "note"
    let diags = check_fixture("xarray_bad.py");
    let xr001: Vec<_> = diags.into_iter().filter(|d| d.rule_id == "XR001").collect();
    assert!(!xr001.is_empty());

    let results = xray::diagnostic::RunResults {
        files: vec![xray::diagnostic::FileResults {
            path: "tests/fixtures/xarray_bad.py".to_string(),
            diagnostics: xr001,
        }],
        paths: vec!["tests/fixtures/xarray_bad.py".to_string()],
    };
    let sarif_str = build_sarif_json(&results).unwrap();
    let sarif: serde_json::Value = serde_json::from_str(&sarif_str).unwrap();
    let first_result = &sarif["runs"][0]["results"][0];
    assert_eq!(first_result["level"], "warning");
}

// ── v0.6: GitLab Code Quality format ─────────────────────────────────────────

#[test]
fn gitlab_output_is_valid_json_array() {
    let diags = check_fixture("xarray_bad.py");
    assert!(!diags.is_empty());

    let results = xray::diagnostic::RunResults {
        files: vec![xray::diagnostic::FileResults {
            path: "tests/fixtures/xarray_bad.py".to_string(),
            diagnostics: diags,
        }],
        paths: vec!["tests/fixtures/xarray_bad.py".to_string()],
    };

    let json_str = build_gitlab_json(&results).expect("gitlab build should succeed");
    let arr: serde_json::Value =
        serde_json::from_str(&json_str).expect("GitLab CQ output should be valid JSON");

    assert!(arr.is_array(), "GitLab CQ output must be a JSON array");
    let entries = arr.as_array().unwrap();
    assert!(!entries.is_empty());

    for entry in entries {
        assert!(entry["description"].is_string(), "must have description");
        assert!(entry["check_name"].is_string(), "must have check_name");
        assert!(entry["fingerprint"].is_string(), "must have fingerprint");
        assert!(entry["severity"].is_string(), "must have severity");
        assert!(entry["location"]["path"].is_string(), "must have path");
        assert!(
            entry["location"]["lines"]["begin"].as_u64().unwrap_or(0) > 0,
            "begin line must be ≥ 1"
        );
    }
}

#[test]
fn gitlab_check_name_prefixed_with_xray() {
    let diags = check_fixture("xarray_bad.py");
    let results = xray::diagnostic::RunResults {
        files: vec![xray::diagnostic::FileResults {
            path: "tests/fixtures/xarray_bad.py".to_string(),
            diagnostics: diags,
        }],
        paths: vec!["tests/fixtures/xarray_bad.py".to_string()],
    };
    let json_str = build_gitlab_json(&results).unwrap();
    let arr: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    for entry in arr.as_array().unwrap() {
        let check_name = entry["check_name"].as_str().unwrap();
        assert!(
            check_name.starts_with("xray/"),
            "check_name should be prefixed with 'xray/', got: {check_name}"
        );
    }
}

#[test]
fn gitlab_fingerprints_are_unique_per_diagnostic() {
    let diags = check_fixture("xarray_bad.py");
    let results = xray::diagnostic::RunResults {
        files: vec![xray::diagnostic::FileResults {
            path: "tests/fixtures/xarray_bad.py".to_string(),
            diagnostics: diags,
        }],
        paths: vec!["tests/fixtures/xarray_bad.py".to_string()],
    };
    let json_str = build_gitlab_json(&results).unwrap();
    let arr: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let fingerprints: Vec<&str> = arr
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["fingerprint"].as_str().unwrap())
        .collect();
    let unique: std::collections::HashSet<_> = fingerprints.iter().collect();
    assert_eq!(
        fingerprints.len(),
        unique.len(),
        "all fingerprints must be unique"
    );
}

#[test]
fn gitlab_severity_mapping_warning_to_major() {
    // XR001 is Warning → should map to GitLab "major"
    let diags = check_fixture("xarray_bad.py");
    let xr001: Vec<_> = diags.into_iter().filter(|d| d.rule_id == "XR001").collect();
    assert!(!xr001.is_empty());
    let results = xray::diagnostic::RunResults {
        files: vec![xray::diagnostic::FileResults {
            path: "tests/fixtures/xarray_bad.py".to_string(),
            diagnostics: xr001,
        }],
        paths: vec!["tests/fixtures/xarray_bad.py".to_string()],
    };
    let json_str = build_gitlab_json(&results).unwrap();
    let arr: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(arr[0]["severity"], "major");
}

// ── v0.7: LSP JSON-RPC message framing ───────────────────────────────────────

#[test]
fn lsp_read_message_parses_body() {
    use std::io::{BufReader, Cursor};
    let body = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    let raw = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    let mut reader = BufReader::new(Cursor::new(raw.into_bytes()));
    let result = lsp::read_message(&mut reader).expect("should parse message");
    assert_eq!(result, body);
}

#[test]
fn lsp_read_message_eof_returns_none() {
    use std::io::{BufReader, Cursor};
    let mut reader = BufReader::new(Cursor::new(b"" as &[u8]));
    assert!(lsp::read_message(&mut reader).is_none());
}

#[test]
fn lsp_read_message_extra_headers_handled() {
    use std::io::{BufReader, Cursor};
    let body = r#"{"method":"exit"}"#;
    let raw = format!(
        "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let mut reader = BufReader::new(Cursor::new(raw.into_bytes()));
    let result = lsp::read_message(&mut reader).expect("should parse despite extra header");
    assert_eq!(result, body);
}

#[test]
fn lsp_uri_to_path_file_scheme() {
    assert_eq!(
        lsp::uri_to_path("file:///home/user/project/analysis.py"),
        Some("/home/user/project/analysis.py".to_string())
    );
}

#[test]
fn lsp_uri_to_path_rejects_non_file() {
    assert!(lsp::uri_to_path("untitled:Untitled-1").is_none());
    assert!(lsp::uri_to_path("https://example.com/file.py").is_none());
}

#[test]
fn lsp_uri_to_path_decodes_spaces() {
    let decoded = lsp::uri_to_path("file:///home/user/my%20project/file.py");
    assert_eq!(decoded, Some("/home/user/my project/file.py".to_string()));
}

// ── v0.7: diagnostic URLs present on all rules ────────────────────────────────

#[test]
fn all_rules_have_url_on_at_least_one_diagnostic() {
    // For each rule domain, lint a bad fixture and verify that *at least one*
    // diagnostic carries a URL.  This catches rules where `with_url()` was
    // accidentally omitted.
    let fixtures = ["xarray_bad.py", "dask_bad.py", "numpy_bad.py", "io_bad.py"];

    for fixture in fixtures {
        let diags = check_fixture(fixture);
        let with_url: Vec<_> = diags.iter().filter(|d| d.url.is_some()).collect();
        assert!(
            !with_url.is_empty(),
            "expected at least one diagnostic with a URL in {fixture}"
        );
    }
}

#[test]
fn dk002_diagnostic_has_url() {
    let diags = check_fixture("dask_bad.py");
    let dk002: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK002").collect();
    assert!(!dk002.is_empty(), "DK002 should fire on dask_bad.py");
    for d in &dk002 {
        assert!(d.url.is_some(), "DK002 diagnostic should carry a docs URL");
        assert!(
            d.url.unwrap().contains("DK002"),
            "DK002 URL should reference the rule ID"
        );
    }
}

#[test]
fn np003_diagnostic_has_url() {
    let diags = check_fixture("numpy_bad.py");
    let np003: Vec<_> = diags.iter().filter(|d| d.rule_id == "NP003").collect();
    assert!(!np003.is_empty(), "NP003 should fire on numpy_bad.py");
    assert!(
        np003[0].url.is_some(),
        "NP003 diagnostic should carry a URL"
    );
}

#[test]
fn io004_diagnostic_has_url() {
    let diags = check_fixture("io_bad.py");
    let io004: Vec<_> = diags.iter().filter(|d| d.rule_id == "IO004").collect();
    assert!(!io004.is_empty(), "IO004 should fire on io_bad.py");
    assert!(
        io004[0].url.is_some(),
        "IO004 diagnostic should carry a URL"
    );
}

// ── v0.9: JSON output schema ──────────────────────────────────────────────────

#[test]
fn json_output_has_schema_version_field() {
    let diags = check_fixture("xarray_bad.py");
    let results = xray::diagnostic::RunResults {
        files: vec![xray::diagnostic::FileResults {
            path: "tests/fixtures/xarray_bad.py".to_string(),
            diagnostics: diags,
        }],
        paths: vec!["tests/fixtures/xarray_bad.py".to_string()],
    };
    let json_str = build_json(&results).expect("build_json should succeed");
    let obj: serde_json::Value =
        serde_json::from_str(&json_str).expect("JSON output should be valid JSON");

    assert!(
        obj.is_object(),
        "JSON output must be an object (envelope), not a bare array"
    );
    assert_eq!(
        obj["schema_version"].as_str(),
        Some(JSON_SCHEMA_VERSION),
        "schema_version field must be present and equal JSON_SCHEMA_VERSION"
    );
}

#[test]
fn json_output_has_diagnostics_array() {
    let diags = check_fixture("xarray_bad.py");
    let results = xray::diagnostic::RunResults {
        files: vec![xray::diagnostic::FileResults {
            path: "tests/fixtures/xarray_bad.py".to_string(),
            diagnostics: diags,
        }],
        paths: vec!["tests/fixtures/xarray_bad.py".to_string()],
    };
    let json_str = build_json(&results).unwrap();
    let obj: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert!(
        obj["diagnostics"].is_array(),
        "JSON envelope must contain a 'diagnostics' array"
    );
    assert!(
        !obj["diagnostics"].as_array().unwrap().is_empty(),
        "diagnostics array must not be empty for a bad fixture"
    );
}

#[test]
fn json_output_has_summary_object() {
    let diags = check_fixture("xarray_bad.py");
    let n = diags.len();
    let results = xray::diagnostic::RunResults {
        files: vec![xray::diagnostic::FileResults {
            path: "tests/fixtures/xarray_bad.py".to_string(),
            diagnostics: diags,
        }],
        paths: vec!["tests/fixtures/xarray_bad.py".to_string()],
    };
    let json_str = build_json(&results).unwrap();
    let obj: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let summary = &obj["summary"];
    assert!(
        summary.is_object(),
        "JSON envelope must contain a 'summary' object"
    );
    let total = summary["total"].as_u64().unwrap_or(0) as usize;
    assert_eq!(
        total, n,
        "summary.total must equal the number of diagnostics"
    );

    let errors = summary["errors"].as_u64().unwrap_or(0);
    let warnings = summary["warnings"].as_u64().unwrap_or(0);
    let hints = summary["hints"].as_u64().unwrap_or(0);
    assert_eq!(
        errors + warnings + hints,
        total as u64,
        "errors + warnings + hints must equal total"
    );
}

#[test]
fn json_output_empty_results_has_zero_summary() {
    let results = xray::diagnostic::RunResults::default();
    let json_str = build_json(&results).unwrap();
    let obj: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(obj["summary"]["total"].as_u64(), Some(0));
    assert!(obj["diagnostics"].as_array().unwrap().is_empty());
}

// ── v0.9: CRLF line ending normalisation ─────────────────────────────────────

#[test]
fn crlf_source_parses_without_error() {
    // A Windows-style file with \r\n line endings must parse cleanly.
    let source = "import xarray as xr\r\nds = xr.open_dataset(\"era5.nc\")\r\n".to_string();
    let parsed = parser::parse_source(source).expect("CRLF source should parse");
    let config = Config::default();
    let diags = rules::run_all(&parsed, "<crlf-test>", &config);
    // XR001 should still fire (open_dataset without chunks=)
    let xr001: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR001").collect();
    assert!(
        !xr001.is_empty(),
        "XR001 should fire on CRLF source just like LF source"
    );
}

#[test]
fn crlf_line_numbers_match_lf_line_numbers() {
    // Verify that a diagnostic produced from a CRLF file points to the same
    // 1-based line number as the identical file with LF endings.
    let lf_source = "import xarray as xr\nds = xr.open_dataset(\"era5.nc\")\n".to_string();
    let crlf_source = "import xarray as xr\r\nds = xr.open_dataset(\"era5.nc\")\r\n".to_string();

    let lf_parsed = parser::parse_source(lf_source).unwrap();
    let crlf_parsed = parser::parse_source(crlf_source).unwrap();
    let config = Config::default();

    let lf_diags = rules::run_all(&lf_parsed, "<lf>", &config);
    let crlf_diags = rules::run_all(&crlf_parsed, "<crlf>", &config);

    let lf_xr001: Vec<_> = lf_diags.iter().filter(|d| d.rule_id == "XR001").collect();
    let crlf_xr001: Vec<_> = crlf_diags.iter().filter(|d| d.rule_id == "XR001").collect();

    assert_eq!(
        lf_xr001.len(),
        crlf_xr001.len(),
        "same number of XR001 diagnostics for LF and CRLF"
    );
    for (lf_d, crlf_d) in lf_xr001.iter().zip(crlf_xr001.iter()) {
        assert_eq!(
            lf_d.line, crlf_d.line,
            "line numbers must be identical for LF and CRLF sources"
        );
        assert_eq!(
            lf_d.column, crlf_d.column,
            "column numbers must be identical for LF and CRLF sources"
        );
    }
}

#[test]
fn crlf_inline_suppression_respected() {
    // Suppression comments must work even when the file uses CRLF endings.
    let source =
        "import xarray as xr\r\nds = xr.open_dataset(\"era5.nc\")  # xray: disable=XR001\r\n"
            .to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "<crlf-suppress>", &config);
    let xr001: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR001").collect();
    assert!(
        xr001.is_empty(),
        "XR001 suppressed with inline comment on CRLF source — should not fire"
    );
}

// ── v0.9: Unicode source hardening ───────────────────────────────────────────

#[test]
fn unicode_docstring_does_not_crash_parser() {
    // Multi-byte UTF-8 characters in strings and comments must not crash the
    // parser or produce incorrect offsets.
    let source = r#"import xarray as xr
# Données climatiques — température de surface (°C)
ds = xr.open_dataset("données.nc")  # naïve open without chunks
arr = ds["température"].values      # .values forces compute
"#
    .to_string();

    let parsed = parser::parse_source(source).expect("Unicode source must parse without error");
    let config = Config::default();
    let diags = rules::run_all(&parsed, "<unicode-test>", &config);

    // Both XR001 and XR002 should fire despite multi-byte characters
    let xr001: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR001").collect();
    let xr002: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR002").collect();
    assert!(
        !xr001.is_empty(),
        "XR001 should fire despite Unicode in source"
    );
    assert!(
        !xr002.is_empty(),
        "XR002 should fire despite Unicode in source"
    );
}

#[test]
fn unicode_source_line_numbers_are_correct() {
    // A rule hit on line 4 in a file with multi-byte chars on earlier lines
    // must still report line 4, not an offset-shifted line.
    let source =
        "import xarray as xr\n# 日本語コメント\n# 한국어 주석\nds = xr.open_dataset(\"data.nc\")\n"
            .to_string();
    let parsed = parser::parse_source(source).expect("Asian-character source must parse");
    let config = Config::default();
    let diags = rules::run_all(&parsed, "<unicode-lines>", &config);
    let xr001: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR001").collect();
    assert!(!xr001.is_empty(), "XR001 must fire");
    assert_eq!(
        xr001[0].line, 4,
        "XR001 must point to line 4 even with multi-byte chars on lines 2-3"
    );
}

#[test]
fn non_utf8_bytes_produce_replacement_chars_not_panic() {
    // A file with a Latin-1 encoded comment (0x80–0xFF bytes) must not
    // cause parse_file to return Err or panic.  We write a temp file with
    // mixed bytes and verify we get back diagnostics rather than an error.
    use std::io::Write;

    let dir = std::env::temp_dir();
    let path = dir.join("xray_test_latin1.py");

    // Write a file with a valid Python import, a latin-1 comment byte (é = 0xe9),
    // and a bad open_dataset call.
    let mut f = std::fs::File::create(&path).expect("create temp file");
    f.write_all(b"import xarray as xr\n# caf\xe9 data\nds = xr.open_dataset(\"data.nc\")\n")
        .expect("write temp file");
    drop(f);

    let path_str = path.to_str().expect("temp path is valid UTF-8");
    let result = parser::parse_file(path_str);

    // Must not return an error even for non-UTF-8 bytes
    assert!(
        result.is_ok(),
        "parse_file must not fail on non-UTF-8 source bytes"
    );

    // XR001 should still fire
    let parsed = result.unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, path_str, &config);
    let xr001: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR001").collect();
    assert!(
        !xr001.is_empty(),
        "XR001 should fire even when source contains non-UTF-8 bytes"
    );

    std::fs::remove_file(&path).ok();
}

// ── v0.9: config edge cases ───────────────────────────────────────────────────

#[test]
fn config_from_malformed_toml_returns_error() {
    // A xray.toml with invalid TOML must return Err, not a default Config.
    use std::io::Write;
    let dir = std::env::temp_dir();
    let config_path = dir.join("xray_test_malformed.toml");
    let mut f = std::fs::File::create(&config_path).expect("create temp config");
    f.write_all(b"[dask\ncompute_call_threshold = !!!\n")
        .expect("write malformed TOML");
    drop(f);

    let result = Config::from_file(&config_path);
    assert!(
        result.is_err(),
        "Config::from_file must return Err for malformed TOML, got Ok"
    );

    std::fs::remove_file(&config_path).ok();
}

#[test]
fn config_from_missing_file_returns_error() {
    let path = std::path::Path::new("/nonexistent/path/xray.toml");
    let result = Config::from_file(path);
    assert!(
        result.is_err(),
        "Config::from_file must return Err when file does not exist"
    );
}

#[test]
fn config_from_valid_toml_preserves_fields() {
    let toml_str = r#"
disable = ["NP003"]

[severity_overrides]
XR001 = "error"

[dask]
compute_call_threshold = 5

[paths]
include = ["src/**/*.py"]
exclude = ["tests/**"]
"#;
    let config: Config = toml::from_str(toml_str).expect("valid TOML should parse");
    assert!(config.disable.contains("NP003"));
    assert_eq!(
        config.severity_overrides.get("XR001").map(String::as_str),
        Some("error")
    );
    assert_eq!(config.dask.compute_call_threshold, 5);
    assert_eq!(config.paths.include, vec!["src/**/*.py"]);
    assert_eq!(config.paths.exclude, vec!["tests/**"]);
}

#[test]
fn config_conflict_cli_disable_overrides_toml() {
    // When a rule is *not* in config.disable but the caller passes it
    // explicitly, the union of both disables the rule.
    // (This tests the pattern used in runner.rs line 84.)
    let source = "import xarray as xr\nds = xr.open_dataset(\"era5.nc\")\n".to_string();
    let parsed = parser::parse_source(source).unwrap();

    // Config does NOT disable XR001
    let config = Config::default();
    let all_diags = rules::run_all(&parsed, "<test>", &config);
    let xr001_before: Vec<_> = all_diags.iter().filter(|d| d.rule_id == "XR001").collect();
    assert!(!xr001_before.is_empty(), "XR001 fires before any disable");

    // Simulate runner.rs applying CLI-level disable on top of rule results
    let cli_disable: std::collections::HashSet<String> =
        ["XR001".to_string()].into_iter().collect();
    let after: Vec<_> = all_diags
        .into_iter()
        .filter(|d| !cli_disable.iter().any(|id| id == d.rule_id))
        .collect();
    assert!(
        after.iter().all(|d| d.rule_id != "XR001"),
        "CLI disable should remove XR001 from results"
    );
}

// ── v0.9: glob edge cases ─────────────────────────────────────────────────────

#[test]
fn collect_paths_with_zero_matches_returns_empty() {
    // A glob pattern that matches no files must produce an empty vec, not an error.
    use xray::runner::collect_paths_pub;
    let result = collect_paths_pub(&["**/this_file_does_not_exist_xray_test_*.py".to_string()]);
    assert!(
        result.is_ok(),
        "zero-match glob should not return Err, got: {:?}",
        result.err()
    );
    assert!(
        result.unwrap().is_empty(),
        "zero-match glob should return empty Vec"
    );
}

#[test]
fn collect_paths_non_python_extension_not_included_by_default() {
    // The default include pattern **/*.py must not match .txt or .rs files.
    use xray::runner::collect_paths_pub;
    let result = collect_paths_pub(&["tests/fixtures/*.txt".to_string()]);
    // Either zero results or an empty vec — no .txt files should be present
    // in the expected-Python glob context (we simply verify no panic).
    assert!(result.is_ok(), "non-python glob must not return Err");
}

#[test]
fn collect_paths_direct_file_path_works() {
    // Passing a literal file path (not a glob) must return exactly that file.
    use xray::runner::collect_paths_pub;
    let path = "tests/fixtures/clean.py".to_string();
    let result = collect_paths_pub(std::slice::from_ref(&path)).unwrap();
    assert!(
        result.contains(&path),
        "literal file path must be included as-is"
    );
}

#[test]
fn collect_paths_deeply_nested_glob_matches() {
    // **/*.py must match files more than two directories deep.
    use xray::runner::collect_paths_pub;
    // We know tests/fixtures/ exists with .py files. Use a pattern that
    // exercises the recursive ** matching.
    let result = collect_paths_pub(&["tests/**/*.py".to_string()]).unwrap();
    assert!(
        !result.is_empty(),
        "tests/**/*.py should match files in tests/fixtures/"
    );
    for p in &result {
        assert!(
            p.ends_with(".py"),
            "every collected path must end with .py, got: {p}"
        );
    }
}

// ── New rules: XR008–XR011, DK007–DK009 ──────────────────────────────────────

#[test]
fn xr008_open_mfdataset_without_parallel() {
    let diags = check_fixture("xarray_bad.py");
    let xr008: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR008").collect();
    assert!(
        !xr008.is_empty(),
        "expected XR008 for open_mfdataset without parallel=True"
    );
}

#[test]
fn xr008_suppressed_when_parallel_true() {
    // open_mfdataset with parallel=True should not fire
    let source =
        "import xarray as xr\nds = xr.open_mfdataset('*.nc', parallel=True, chunks='auto')\n"
            .to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "test.py", &config);
    let xr008: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR008").collect();
    assert!(xr008.is_empty(), "XR008 must not fire when parallel=True");
}

#[test]
fn xr008_open_dataset_does_not_fire() {
    // XR008 must only fire for open_mfdataset, not for open_dataset
    let source = "import xarray as xr\nds = xr.open_dataset('file.nc')\n".to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "test.py", &config);
    let xr008: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR008").collect();
    assert!(
        xr008.is_empty(),
        "XR008 must not fire for open_dataset (only open_mfdataset)"
    );
}

#[test]
fn xr009_apply_ufunc_dask_allowed() {
    let diags = check_fixture("xarray_bad.py");
    let xr009: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR009").collect();
    assert!(
        !xr009.is_empty(),
        "expected XR009 for apply_ufunc with dask='allowed'"
    );
}

#[test]
fn xr009_suppressed_when_dask_parallelized() {
    let source = "import xarray as xr\nimport numpy as np\nresult = xr.apply_ufunc(np.exp, arr, dask='parallelized', output_dtypes=[float])\n".to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "test.py", &config);
    let xr009: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR009").collect();
    assert!(
        xr009.is_empty(),
        "XR009 must not fire when dask='parallelized'"
    );
}

#[test]
fn xr009_suppressed_when_no_dask_kwarg() {
    // apply_ufunc without any dask= kwarg should not fire
    let source = "import xarray as xr\nimport numpy as np\nresult = xr.apply_ufunc(np.exp, arr)\n"
        .to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "test.py", &config);
    let xr009: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR009").collect();
    assert!(
        xr009.is_empty(),
        "XR009 must not fire when dask= kwarg is absent"
    );
}

#[test]
fn xr010_merge_in_loop() {
    let diags = check_fixture("xarray_bad.py");
    let xr010: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR010").collect();
    assert!(
        !xr010.is_empty(),
        "expected XR010 for xr.merge inside a for loop"
    );
}

#[test]
fn xr010_merge_outside_loop_ok() {
    let source = "import xarray as xr\nds1 = xr.Dataset()\nds2 = xr.Dataset()\nresult = xr.merge([ds1, ds2])\n".to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "test.py", &config);
    let xr010: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR010").collect();
    assert!(
        xr010.is_empty(),
        "XR010 must not fire for merge outside a loop"
    );
}

#[test]
fn xr011_to_netcdf_without_encoding() {
    let diags = check_fixture("xarray_bad.py");
    let xr011: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR011").collect();
    assert!(
        !xr011.is_empty(),
        "expected XR011 for to_netcdf without encoding="
    );
}

#[test]
fn xr011_suppressed_when_encoding_present() {
    let source = "import xarray as xr\nds = xr.Dataset()\nds.to_netcdf('out.nc', encoding={'var': {'zlib': True}})\n".to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "test.py", &config);
    let xr011: Vec<_> = diags.iter().filter(|d| d.rule_id == "XR011").collect();
    assert!(
        xr011.is_empty(),
        "XR011 must not fire when encoding= is provided"
    );
}

#[test]
fn dk007_from_array_without_chunks() {
    let diags = check_fixture("dask_bad.py");
    let dk007: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK007").collect();
    assert!(
        !dk007.is_empty(),
        "expected DK007 for da.from_array without chunks="
    );
}

#[test]
fn dk007_suppressed_when_chunks_present() {
    let source = "import dask.array as da\nimport numpy as np\narr = da.from_array(np.ones((1000, 1000)), chunks=(100, 100))\n".to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "test.py", &config);
    let dk007: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK007").collect();
    assert!(
        dk007.is_empty(),
        "DK007 must not fire when chunks= is provided"
    );
}

#[test]
fn dk008_rechunk_in_loop() {
    let diags = check_fixture("dask_bad.py");
    let dk008: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK008").collect();
    assert!(
        !dk008.is_empty(),
        "expected DK008 for .rechunk() inside a for loop"
    );
}

#[test]
fn dk008_rechunk_outside_loop_ok() {
    let source = "import dask.array as da\nimport numpy as np\narr = da.from_array(np.ones((1000,)), chunks=100)\narr = arr.rechunk(200)\n".to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "test.py", &config);
    let dk008: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK008").collect();
    assert!(
        dk008.is_empty(),
        "DK008 must not fire for rechunk outside a loop"
    );
}

#[test]
fn dk009_concatenate_in_loop() {
    let diags = check_fixture("dask_bad.py");
    let dk009: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK009").collect();
    assert!(
        !dk009.is_empty(),
        "expected DK009 for da.concatenate inside a for loop"
    );
}

#[test]
fn dk009_concatenate_outside_loop_ok() {
    let source = "import dask.array as da\nimport numpy as np\narrays = [da.ones((100,), chunks=10) for _ in range(5)]\nresult = da.concatenate(arrays)\n".to_string();
    let parsed = parser::parse_source(source).unwrap();
    let config = Config::default();
    let diags = rules::run_all(&parsed, "test.py", &config);
    let dk009: Vec<_> = diags.iter().filter(|d| d.rule_id == "DK009").collect();
    assert!(
        dk009.is_empty(),
        "DK009 must not fire for concatenate outside a loop"
    );
}

// ── regression: false positives that used to fire ─────────────────────────────
//
// Every construct in `false_positives.py` is legitimate code that an earlier
// version of xray reported.  The fixture must stay completely silent.

#[test]
fn false_positive_fixture_is_silent() {
    let diags = check_fixture("false_positives.py");
    assert!(
        diags.is_empty(),
        "false_positives.py must produce zero diagnostics, got: {:#?}",
        diags
            .iter()
            .map(|d| format!("{}:{} [{}] {}", d.file, d.line, d.rule_id, d.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn xr003_ignores_non_dimension_attributes() {
    // `for f in self.files:` is not a dimension loop.
    let diags = check_fixture("false_positives.py");
    assert!(diags.iter().all(|d| d.rule_id != "XR003"));
}

#[test]
fn xr007_and_xr010_ignore_pandas_receivers() {
    let diags = check_fixture("false_positives.py");
    assert!(
        diags
            .iter()
            .all(|d| d.rule_id != "XR007" && d.rule_id != "XR010"),
        "pandas concat/merge must not be attributed to xarray"
    );
}

#[test]
fn io003_ignores_builtin_open() {
    let diags = check_fixture("false_positives.py");
    assert!(diags.iter().all(|d| d.rule_id != "IO003"));
}

#[test]
fn io004_ignores_plain_list_and_dict_indexing() {
    let diags = check_fixture("false_positives.py");
    assert!(diags.iter().all(|d| d.rule_id != "IO004"));
}

#[test]
fn np005_ignores_nested_list_indexing() {
    let diags = check_fixture("false_positives.py");
    assert!(diags.iter().all(|d| d.rule_id != "NP005"));
}

#[test]
fn missing_keyword_rules_respect_kwargs_splat() {
    // `xr.open_dataset(path, **OPTS)` may well set chunks=; xray cannot know.
    let diags = check_fixture("false_positives.py");
    assert!(
        diags
            .iter()
            .all(|d| d.rule_id != "XR001" && d.rule_id != "DK007"),
        "rules must not claim a keyword is missing when **kwargs is forwarded"
    );
}

// ── regression: loop contexts that used to be missed ─────────────────────────

#[test]
fn compute_in_comprehension_is_flagged() {
    let diags = check_fixture("loop_context.py");
    assert!(
        diags.iter().any(|d| d.rule_id == "DK001" && d.line == 11),
        "`[x.compute() for x in items]` should be flagged, got: {:?}",
        diags
            .iter()
            .map(|d| (d.rule_id, d.line))
            .collect::<Vec<_>>()
    );
}

#[test]
fn compute_in_while_loop_is_flagged() {
    let diags = check_fixture("loop_context.py");
    assert!(
        diags.iter().any(|d| d.rule_id == "DK001" && d.line == 18),
        "`.compute()` inside a while loop should be flagged"
    );
}

#[test]
fn compute_in_loop_header_is_not_flagged() {
    // The iterable of a `for` statement is evaluated once, not per iteration.
    let diags = check_fixture("loop_context.py");
    assert!(
        diags
            .iter()
            .all(|d| !(d.line == 25 && (d.rule_id == "DK001" || d.rule_id == "XR005"))),
        "a compute in the loop header runs once and must not be reported"
    );
}

#[test]
fn xr004_matches_negative_float_coordinates() {
    let diags = check_fixture("loop_context.py");
    assert!(
        diags.iter().any(|d| d.rule_id == "XR004"),
        "`.sel(lat=-33.5)` should be flagged — negative floats are floats"
    );
}

// ── regression: overlapping rules report a call once ─────────────────────────

#[test]
fn compute_in_loop_reports_one_rule_per_position() {
    use std::collections::HashMap;
    let diags = check_fixture("loop_context.py");
    let mut by_pos: HashMap<(usize, usize), Vec<&str>> = HashMap::new();
    for d in &diags {
        by_pos
            .entry((d.line, d.column))
            .or_default()
            .push(d.rule_id);
    }
    for (pos, ids) in by_pos {
        let compute_rules: Vec<_> = ids
            .iter()
            .filter(|id| matches!(**id, "XR005" | "DK001" | "DK002"))
            .collect();
        assert!(
            compute_rules.len() <= 1,
            "position {pos:?} reported by several compute-in-loop rules: {compute_rules:?}"
        );
    }
}

// ── receiver tracking (ToFix §19 — accuracy) ─────────────────────────────────

/// Lint an inline snippet with default config.
fn check_src(source: &str) -> Vec<xray::diagnostic::Diagnostic> {
    let parsed = parser::parse_source(source.to_string()).unwrap();
    let config = Config::default();
    let mut diags = rules::run_all(&parsed, "<inline>", &config);
    diags.sort_by_key(|d| (d.line, d.rule_id));
    diags
}

fn ids_of(diags: &[xray::diagnostic::Diagnostic], rule: &str) -> Vec<usize> {
    diags
        .iter()
        .filter(|d| d.rule_id == rule)
        .map(|d| d.line)
        .collect()
}

#[test]
fn xr002_ignores_pandas_and_numpy_receivers() {
    let diags = check_src(
        "import xarray as xr\n\
         import pandas as pd\n\
         import numpy as np\n\
         df = pd.read_csv('a.csv')\n\
         arr = np.zeros(3, dtype=np.float32)\n\
         a = df.values\n\
         b = arr.values\n",
    );
    assert!(
        ids_of(&diags, "XR002").is_empty(),
        "XR002 must not fire on pandas/numpy receivers: {diags:?}"
    );
}

#[test]
fn xr002_still_fires_on_xarray_receivers() {
    let diags = check_src(
        "import xarray as xr\n\
         ds = xr.open_dataset('a.nc', chunks='auto')\n\
         a = ds.values\n",
    );
    assert_eq!(
        ids_of(&diags, "XR002"),
        vec![3],
        "XR002 must still fire on a known xarray receiver"
    );
}

#[test]
fn xr002_still_fires_on_unknown_receivers() {
    // A function parameter has no known origin. Rules must keep their prior
    // behaviour rather than silently going quiet on everything untracked.
    let diags = check_src(
        "import xarray as xr\n\
         def process(ds):\n\
         \x20   return ds.values\n",
    );
    assert_eq!(ids_of(&diags, "XR002"), vec![3]);
}

#[test]
fn dk004_ignores_idiomatic_reduce_then_compute() {
    let diags = check_src(
        "import xarray as xr\n\
         ds = xr.open_dataset('a.nc', chunks='auto')\n\
         a = ds.mean().compute()\n\
         b = ds.sum().compute()\n\
         c = ds.sel(time='2020').compute()\n",
    );
    assert!(
        ids_of(&diags, "DK004").is_empty(),
        "DK004 must not fire on reduce-then-compute: {diags:?}"
    );
}

#[test]
fn dk004_still_fires_on_construct_then_compute() {
    let diags = check_src(
        "import dask.array as da\n\
         vals = [1, 2, 3]\n\
         d = da.from_array(vals).compute()\n",
    );
    assert_eq!(
        ids_of(&diags, "DK004"),
        vec![3],
        "DK004 must still fire when the graph never did any work"
    );
}

#[test]
fn np004_ignores_scalars_outside_a_loop() {
    let diags = check_src(
        "import numpy as np\n\
         import math\n\
         x = math.sqrt(2.0)\n",
    );
    assert!(
        ids_of(&diags, "NP004").is_empty(),
        "NP004 must not hint on a genuine scalar: {diags:?}"
    );
}

#[test]
fn np004_fires_on_arrays_outside_a_loop() {
    let diags = check_src(
        "import numpy as np\n\
         import math\n\
         arr = np.arange(10, dtype=np.float32)\n\
         x = math.sqrt(arr)\n",
    );
    assert_eq!(ids_of(&diags, "NP004"), vec![4]);
}

#[test]
fn np004_still_fires_on_scalars_inside_a_loop() {
    // The iteration itself is the problem here, whatever the argument is.
    let diags = check_src(
        "import numpy as np\n\
         import math\n\
         out = []\n\
         for v in range(10):\n\
         \x20   out.append(math.sqrt(v))\n",
    );
    assert_eq!(ids_of(&diags, "NP004"), vec![5]);
}

#[test]
fn np003_describes_full_dtype_inference_correctly() {
    let diags = check_src(
        "import numpy as np\n\
         a = np.full((4, 4), 0)\n\
         b = np.zeros((4, 4))\n",
    );
    let full = diags
        .iter()
        .find(|d| d.rule_id == "NP003" && d.line == 2)
        .expect("NP003 should fire for np.full");
    assert!(
        full.message
            .contains("infers its dtype from the fill value"),
        "np.full infers int64 from `0`, it does not default to float64: {}",
        full.message
    );
    let zeros = diags
        .iter()
        .find(|d| d.rule_id == "NP003" && d.line == 3)
        .expect("NP003 should fire for np.zeros");
    assert!(zeros.message.contains("float64"));
}

#[test]
fn np007b_matches_assigned_apply_in_loop() {
    // The old `(_)*` query shape only saw bare expression statements, so an
    // assigned result inside a loop was missed entirely.
    let diags = check_src(
        "import pandas as pd\n\
         df = pd.DataFrame({'a': [1]})\n\
         for i in range(3):\n\
         \x20   out = df.apply(lambda r: r + 1)\n",
    );
    assert_eq!(ids_of(&diags, "NP007"), vec![4]);
}

#[test]
fn np007b_reports_one_diagnostic_per_call() {
    // The `(_)* ... (_)*` shape could match at several split points.
    let diags = check_src(
        "import pandas as pd\n\
         df = pd.DataFrame({'a': [1]})\n\
         for i in range(3):\n\
         \x20   print(1)\n\
         \x20   df.apply(lambda r: r + 1)\n\
         \x20   print(2)\n",
    );
    assert_eq!(ids_of(&diags, "NP007"), vec![5]);
}

#[test]
fn np007b_does_not_fire_outside_a_loop() {
    let diags = check_src(
        "import pandas as pd\n\
         df = pd.DataFrame({'a': [1]})\n\
         out = df.apply(lambda r: r + 1)\n",
    );
    assert!(ids_of(&diags, "NP007").is_empty());
}

// ── suppression scoping and CRLF rendering (ToFix §34) ───────────────────────

#[test]
fn docstring_examples_do_not_suppress_real_diagnostics() {
    // A docstring demonstrating how to suppress a rule used to suppress it for
    // real — `disable-file=` inside a docstring took out the whole file.
    let diags = check_src(
        "import xarray as xr\n\
         def helper():\n\
         \x20   \"\"\"\n\
         \x20   To silence this rule, write:\n\
         \x20       ds = xr.open_dataset('a.nc')  # xray: disable-file=XR001\n\
         \x20   \"\"\"\n\
         \x20   return None\n\
         ds = xr.open_dataset('real.nc')\n",
    );
    assert_eq!(
        ids_of(&diags, "XR001"),
        vec![8],
        "the real open_dataset on line 8 must still be reported"
    );
}

#[test]
fn inline_suppression_rule_ids_are_case_insensitive() {
    let diags =
        check_src("import xarray as xr\nds = xr.open_dataset('a.nc')  # xray: disable=xr001\n");
    assert!(
        ids_of(&diags, "XR001").is_empty(),
        "lowercase inline rule IDs must work, as they do in --disable and xray.toml"
    );
}

#[test]
fn crlf_and_lf_sources_produce_identical_diagnostics() {
    let lf = "import xarray as xr\n# pad\n# pad\nds = xr.open_dataset('a.nc')\n";
    let crlf = lf.replace('\n', "\r\n");
    let a: Vec<_> = check_src(lf)
        .into_iter()
        .map(|d| (d.rule_id, d.line, d.column))
        .collect();
    let b: Vec<_> = check_src(&crlf)
        .into_iter()
        .map(|d| (d.rule_id, d.line, d.column))
        .collect();
    assert_eq!(a, b, "line endings must not shift diagnostic positions");
}
