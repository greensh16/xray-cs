/// Per-rule explanations shown by `xray explain <RULE_ID>`.
pub struct ExplainEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub severity: &'static str,
    pub domain: &'static str,
    pub rationale: &'static str,
    pub bad_example: &'static str,
    pub good_example: &'static str,
    pub url: Option<&'static str>,
    /// Is there a mechanical, copy-paste-ready fix for this rule?
    pub fix_eligible: bool,
}

/// The entry for a rule ID, if one exists. Case-insensitive.
pub fn entry_for(rule_id: &str) -> Option<&'static ExplainEntry> {
    let upper = rule_id.to_uppercase();
    ENTRIES.iter().find(|e| e.id == upper.as_str())
}

/// Print a formatted explanation for the given rule ID.
/// Returns `false` if the rule ID is unknown.
pub fn explain(rule_id: &str) -> bool {
    let id_upper = rule_id.to_uppercase();
    match ENTRIES.iter().find(|e| e.id == id_upper.as_str()) {
        None => {
            eprintln!("xray: unknown rule `{rule_id}`. Run `xray --list-rules` to see all rules.");
            false
        }
        Some(e) => {
            print_entry(e);
            true
        }
    }
}

fn print_entry(e: &ExplainEntry) {
    let sep = "─".repeat(72);
    println!();
    println!("  {sep}");
    println!("  {} · {}  [{}]  ({})", e.id, e.name, e.severity, e.domain);
    println!("  {sep}");
    println!();
    println!("  WHY THIS MATTERS");
    for line in e.rationale.lines() {
        println!("    {line}");
    }
    println!();
    println!("  ❌  BAD EXAMPLE");
    for line in e.bad_example.lines() {
        println!("    {line}");
    }
    println!();
    println!("  ✅  GOOD EXAMPLE");
    for line in e.good_example.lines() {
        println!("    {line}");
    }
    if e.fix_eligible {
        println!();
        println!("  🔧  AUTO-FIX ELIGIBLE — xray emits a `fix_hint` in JSON output");
    }
    if let Some(url) = e.url {
        println!();
        println!("  📖  DOCS");
        println!("    {url}");
    }
    println!();
    println!("  {sep}");
    println!();
}

/// All rule explanation entries, in domain order.
static ENTRIES: &[ExplainEntry] = &[
    // ── xarray ────────────────────────────────────────────────────────────────
    ExplainEntry {
        id: "XR000",
        name: "stale-suppression",
        severity: "hint",
        domain: "suppressions",
        rationale: "\
A `# xray: disable=RULE` comment that suppressed nothing is dead weight, and
worse than dead: the line it guards will change, and the comment will go on
silencing whatever that line becomes.  Nothing else reports these, so they
accumulate quietly as the code under them moves on.

Only line-level suppressions are checked.  `disable-file=` legitimately covers a
file that happens to have no violations right now, and flagging it would punish
exactly the defensive use it exists for.",
        bad_example: "\
# the loop was refactored away; the suppression outlived it
total = ds.sum()  # xray: disable=DK001",
        good_example: "\
total = ds.sum()",
        url: Some("https://github.com/greensh16/xray-cs/wiki/Suppressions"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "XR001",
        name: "open-dataset-without-chunks",
        severity: "warning",
        domain: "xarray",
        rationale: "\
xr.open_dataset() and xr.open_mfdataset() load data eagerly into memory when
called without chunks=.  On HPC systems with multi-TB datasets this causes
out-of-memory errors and blocks the entire Python process until the read
completes.  Passing chunks= wraps the array in a dask graph so reads stay
lazy and distributed.",
        bad_example: "\
ds = xr.open_dataset(\"era5_1979.nc\")           # eager — loads ~4 GB now
ds_multi = xr.open_mfdataset(\"era5_*.nc\")      # eager — loads all files",
        good_example: "\
ds = xr.open_dataset(\"era5_1979.nc\", chunks={\"time\": 24, \"lat\": 181})
ds_multi = xr.open_mfdataset(\"era5_*.nc\", chunks=\"auto\")",
        url: Some("https://docs.xarray.dev/en/stable/user-guide/dask.html"),
        fix_eligible: true,
    },
    ExplainEntry {
        id: "XR002",
        name: "values-access-on-dataarray",
        severity: "warning",
        domain: "xarray",
        rationale: "\
Accessing .values on an xarray DataArray materialises the entire backing
array into a plain NumPy ndarray, discarding all coordinate labels, dimension
names, and CF metadata.  This is almost never intentional and forces the full
dask compute graph to execute immediately.",
        bad_example: "\
arr = ds[\"u10\"].values       # drops all coordinate metadata, triggers compute",
        good_example: "\
arr = ds[\"u10\"].to_numpy()   # explicit and readable
arr = ds[\"u10\"].data         # keeps dask arrays lazy",
        url: Some("https://docs.xarray.dev/en/stable/generated/xarray.DataArray.to_numpy.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "XR003",
        name: "loop-over-dimension",
        severity: "hint",
        domain: "xarray",
        rationale: "\
Iterating over a Dataset dimension attribute (e.g. `for t in ds.time`) in a
Python for-loop bypasses xarray's vectorised operations and forces Python-level
dispatch on every element.  For large dimensions this is 10-1000× slower than
the equivalent isel/sel call.",
        bad_example: "\
for t in ds.time:
    print(t)          # Python loop over potentially thousands of timestamps",
        good_example: "\
n = ds.sizes[\"time\"]
subset = ds.isel(time=slice(0, n // 2))   # vectorised — no Python loop",
        url: Some("https://docs.xarray.dev/en/stable/user-guide/computation.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "XR004",
        name: "sel-with-float",
        severity: "warning",
        domain: "xarray",
        rationale: "\
xarray's .sel() uses exact equality by default when given a float value.
Floating-point coordinate comparison almost always fails silently — you get
an empty result rather than an error.  Pass method='nearest' or tolerance=
to perform inexact matching.",
        bad_example: "\
point = ds.sel(lat=45.0, lon=-120.5)    # likely returns empty DataArray",
        good_example: "\
point = ds.sel(lat=45.0, lon=-120.5, method=\"nearest\")
point = ds.sel(lat=45.0, lon=-120.5, tolerance=0.01)",
        url: Some("https://docs.xarray.dev/en/stable/generated/xarray.Dataset.sel.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "XR005",
        name: "compute-in-loop",
        severity: "error",
        domain: "xarray",
        rationale: "\
Calling .compute() inside a for loop rebuilds and executes the entire dask
task graph on every iteration.  If you have N iterations this is O(N) full
graph executions where O(1) would suffice.  Call .persist() before the loop
to keep the hot result in distributed memory.",
        bad_example: "\
for year in range(2000, 2024):
    result = ds.sel(time=str(year)).compute()   # full graph on every iteration",
        good_example: "\
ds_hot = ds.persist()   # materialise once
for year in range(2000, 2024):
    result = ds_hot.sel(time=str(year)).compute()   # cheap slice of hot data",
        url: Some("https://docs.dask.org/en/stable/best-practices.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "XR006",
        name: "to-array-without-dim",
        severity: "warning",
        domain: "xarray",
        rationale: "\
Calling .to_array() or .to_dataarray() without dim= silently creates a new
dimension named 'variable'.  Downstream code that references this dimension
by name will break if the variable names ever change, or when collaborators
reading the code don't know the implicit name.",
        bad_example: "\
stacked = ds.to_array()           # new dim called 'variable' — implicit
stacked2 = ds.to_dataarray()      # same issue",
        good_example: "\
stacked = ds.to_array(dim=\"variable\")    # explicit — intent is clear",
        url: Some("https://docs.xarray.dev/en/stable/generated/xarray.Dataset.to_array.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "XR007",
        name: "concat-in-loop",
        severity: "error",
        domain: "xarray",
        rationale: "\
xr.concat inside a for loop creates O(n²) copies: each concatenation must
copy all previously concatenated data.  For n=100 slices this is ~5000
unnecessary array copies.  Collect first, concat once.",
        bad_example: "\
combined = ds.isel(time=0)
for i in range(1, 100):
    combined = xr.concat([combined, ds.isel(time=i)], dim=\"time\")  # O(n²)",
        good_example: "\
slices = [ds.isel(time=i) for i in range(100)]
combined = xr.concat(slices, dim=\"time\")   # single pass",
        url: Some("https://docs.xarray.dev/en/stable/generated/xarray.concat.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "XR008",
        name: "open-mfdataset-without-parallel",
        severity: "warning",
        domain: "xarray",
        rationale: "\
xr.open_mfdataset opens files one-by-one using the default serial engine
when parallel=True is not passed.  On large multi-file ensembles (hundreds
of ERA5 files, for example) this can take minutes where parallel opening
via dask.delayed would take seconds.  parallel=True is available in all
xarray versions >= 0.10.",
        bad_example: "\
ds = xr.open_mfdataset(sorted(glob.glob(\"era5_*.nc\")), chunks=\"auto\")
# opens ~8760 hourly files serially — can take 5-10 min on Gadi",
        good_example: "\
ds = xr.open_mfdataset(sorted(glob.glob(\"era5_*.nc\")),
                       parallel=True, chunks=\"auto\")",
        url: Some("https://docs.xarray.dev/en/stable/generated/xarray.open_mfdataset.html"),
        fix_eligible: true,
    },
    ExplainEntry {
        id: "XR009",
        name: "apply-ufunc-dask-allowed",
        severity: "warning",
        domain: "xarray",
        rationale: "\
xr.apply_ufunc with dask='allowed' silently falls back to executing the
function on the underlying NumPy array when a dask-backed DataArray is
passed.  This calls dask.compute() internally, collapsing the lazy graph
and running serial NumPy code.  Use dask='parallelized' to keep execution
distributed; pair it with output_dtypes=[...] to let xarray infer the
output chunk layout without executing.",
        bad_example: "\
result = xr.apply_ufunc(np.exp, ds[\"u10\"], dask=\"allowed\")
# silently collapses the dask graph — runs serial NumPy on full array",
        good_example: "\
result = xr.apply_ufunc(
    np.exp, ds[\"u10\"],
    dask=\"parallelized\",
    output_dtypes=[ds[\"u10\"].dtype],
)",
        url: Some("https://docs.xarray.dev/en/stable/generated/xarray.apply_ufunc.html"),
        fix_eligible: true,
    },
    ExplainEntry {
        id: "XR010",
        name: "merge-in-loop",
        severity: "warning",
        domain: "xarray",
        rationale: "\
xr.merge inside a for loop pays the full alignment and coordinate
broadcasting cost on every iteration.  Each call must reconcile dimension
coordinates across all datasets seen so far, making the overall complexity
O(n²) in the number of iterations.  Collect datasets first, then merge once.",
        bad_example: "\
merged = xr.Dataset()
for year in range(2000, 2020):
    merged = xr.merge([merged, annual[year]])   # O(n²) alignment",
        good_example: "\
datasets = [annual[year] for year in range(2000, 2020)]
merged = xr.merge(datasets)   # single alignment pass",
        url: Some("https://docs.xarray.dev/en/stable/generated/xarray.merge.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "XR011",
        name: "to-netcdf-without-encoding",
        severity: "hint",
        domain: "xarray",
        rationale: "\
Without encoding= xarray writes each variable at its native in-memory dtype
(typically float64) with no compression.  A typical ERA5 variable at float64
is about 2× the size of float32 and 10× the size of an int16 with
scale/offset.  Adding zlib=True alone usually halves file size; switching to
float32 halves it again, with no loss of precision for most meteorological
quantities.",
        bad_example: "\
ds.to_netcdf(\"output.nc\")
# u10 written as float64, no compression — typical 5-10× larger than needed",
        good_example: "\
encoding = {
    \"u10\": {\"dtype\": \"float32\", \"zlib\": True, \"complevel\": 4},
    \"v10\": {\"dtype\": \"float32\", \"zlib\": True, \"complevel\": 4},
}
ds.to_netcdf(\"output.nc\", encoding=encoding)",
        url: Some("https://docs.xarray.dev/en/stable/user-guide/io.html#writing-encoded-data"),
        fix_eligible: false,
    },
    // ── dask ──────────────────────────────────────────────────────────────────
    ExplainEntry {
        id: "DK001",
        name: "compute-in-for-loop",
        severity: "error",
        domain: "dask",
        rationale: "\
Calling .compute() inside a for loop materialises the full dask task graph
on every iteration.  This negates the lazy evaluation benefit and often
causes out-of-memory conditions when intermediate results pile up.",
        bad_example: "\
for i in range(10):
    chunk = da.from_array(data[i], chunks=50)
    result = chunk.mean().compute()   # full rebuild every iteration",
        good_example: "\
chunks = [da.from_array(data[i], chunks=50).mean() for i in range(10)]
results = dask.compute(*chunks)   # single scheduler dispatch",
        url: Some("https://docs.dask.org/en/stable/best-practices.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "DK002",
        name: "dask-compute-in-for-loop",
        severity: "error",
        domain: "dask",
        rationale: "\
dask.compute() called inside a for loop serialises execution — each
iteration blocks until the previous one completes, throwing away dask's
ability to run tasks in parallel.",
        bad_example: "\
for item in delayed_items:
    val = dask.compute(item)   # serial, one at a time",
        good_example: "\
results = dask.compute(*delayed_items)   # parallel batch",
        url: Some("https://docs.dask.org/en/stable/api.html#dask.compute"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "DK003",
        name: "excessive-compute-calls",
        severity: "warning",
        domain: "dask",
        rationale: "\
Multiple .compute() calls in the same scope each trigger a full scheduler
round-trip.  If the intermediate arrays are reused, .persist() keeps the
result in distributed memory so subsequent operations are faster.",
        bad_example: "\
r1 = a.sum().compute()
r2 = a.mean().compute()    # second full compute — a is recomputed from scratch
r3 = a.std().compute()     # third full compute",
        good_example: "\
a_hot = a.persist()
r1, r2, r3 = dask.compute(a_hot.sum(), a_hot.mean(), a_hot.std())",
        url: Some("https://docs.dask.org/en/stable/api.html#dask.persist"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "DK004",
        name: "immediate-compute",
        severity: "hint",
        domain: "dask",
        rationale: "\
Constructing a dask array and immediately calling .compute() on it in the
same expression means the lazy graph is never reused.  The overhead of
building the task graph outweighs any benefit — use NumPy/pandas directly.

Only constructors and loaders count: from_array, from_delayed, from_pandas,
read_csv, open_dataset and friends.  A reduction such as ds.mean().compute()
is the correct idiom — dask performed the parallel work and .compute() simply
retrieves the small result — so it is not flagged.",
        bad_example: "\
result = da.from_array(np.arange(1000), chunks=100).compute()  # never lazy",
        good_example: "\
result = np.arange(1000)   # if you always compute immediately, skip dask",
        url: Some("https://docs.dask.org/en/stable/best-practices.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "DK005",
        name: "persist-result-discarded",
        severity: "warning",
        domain: "dask",
        rationale: "\
.persist() schedules work on the cluster and returns a future-like object.
Calling it as a standalone statement discards this object — the cluster
pays the cost of executing the graph but the result is immediately garbage-
collected, wasting compute time and memory bandwidth.",
        bad_example: "\
a.persist()   # result discarded — cluster does work, you get nothing",
        good_example: "\
a_hot = a.persist()   # assign the result; use a_hot in subsequent ops
x = a_hot.sum()
y = a_hot.mean()",
        url: Some("https://docs.dask.org/en/stable/api.html#dask.persist"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "DK006",
        name: "persist-then-compute",
        severity: "warning",
        domain: "dask",
        rationale: "\
.persist().compute() sends the computation to the cluster (.persist) and
then immediately blocks until it comes back (.compute).  The round-trip adds
latency without benefit — use .compute() alone, or .persist() and reuse
the result across multiple operations before computing.",
        bad_example: "\
result = a.persist().compute()   # pointless cluster round-trip",
        good_example: "\
result = a.compute()             # direct, no spurious persist
# — or —
a_hot = a.persist()              # keep distributed if reused
r1 = a_hot.sum().compute()
r2 = a_hot.mean().compute()",
        url: Some("https://docs.dask.org/en/stable/best-practices.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "DK007",
        name: "from-array-without-chunks",
        severity: "warning",
        domain: "dask",
        rationale: "\
da.from_array() without chunks= places the entire array in a single
partition.  A single-chunk array has a graph with no parallelism — every
operation on it runs in one thread.  Worse, the full array must fit in a
single worker's memory, losing dask's distributed benefit entirely.  Always
specify chunks= explicitly.",
        bad_example: "\
import dask.array as da
import numpy as np
arr = da.from_array(np.random.rand(50_000, 50_000))
# one 20 GB chunk — no parallelism possible",
        good_example: "\
arr = da.from_array(np.random.rand(50_000, 50_000), chunks=(5_000, 5_000))
# 100 chunks of 200 MB — can be processed by 100 workers in parallel",
        url: Some("https://docs.dask.org/en/stable/array-creation.html"),
        fix_eligible: true,
    },
    ExplainEntry {
        id: "DK008",
        name: "rechunk-in-loop",
        severity: "warning",
        domain: "dask",
        rationale: "\
.rechunk() rearranges the task graph to use a new chunk layout.  Inside a
for loop each call triggers a full re-partition of the accumulated data —
O(n) rechunks for n iterations.  Determine the target chunk layout once
before the loop and rechunk a single time.",
        bad_example: "\
for step in range(100):
    data = data.rechunk({0: 200})   # 100 graph rebuilds — very slow",
        good_example: "\
data = data.rechunk({0: 200})   # rechunk once before the loop
for step in range(100):
    data = process(data)",
        url: Some("https://docs.dask.org/en/stable/array-best-practices.html#rechunking"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "DK009",
        name: "concatenate-in-loop",
        severity: "error",
        domain: "dask",
        rationale: "\
da.concatenate() inside a for loop creates O(n²) intermediate copies, just
like np.concatenate or xr.concat in a loop.  Each iteration must copy all
previously concatenated data.  For n=100 arrays of size 1 MB each the loop
produces ~5 GB of intermediate data; collecting and concatenating once
produces ~100 MB.",
        bad_example: "\
acc = da.zeros((0,), chunks=100)
for i in range(100):
    acc = da.concatenate([acc, da.ones((1000,), chunks=100)])  # O(n²)",
        good_example: "\
arrays = [da.ones((1000,), chunks=100) for _ in range(100)]
acc = da.concatenate(arrays)   # single O(n) pass",
        url: Some("https://docs.dask.org/en/stable/array-api.html#dask.array.concatenate"),
        fix_eligible: false,
    },
    // ── numpy / pandas ────────────────────────────────────────────────────────
    ExplainEntry {
        id: "NP001",
        name: "iterrows",
        severity: "warning",
        domain: "numpy/pandas",
        rationale: "\
DataFrame.iterrows() iterates row-by-row in Python, running the interpreter
overhead on every row.  For a 1M-row DataFrame this is typically 100-1000×
slower than the equivalent vectorised operation.",
        bad_example: "\
for idx, row in df.iterrows():
    totals.append(row[\"a\"] + row[\"b\"])",
        good_example: "\
totals = df[\"a\"] + df[\"b\"]   # vectorised — no Python loop",
        url: Some("https://pandas.pydata.org/docs/user_guide/enhancingperf.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "NP002",
        name: "concat-in-loop",
        severity: "error",
        domain: "numpy/pandas",
        rationale: "\
pd.concat or np.concatenate inside a loop creates O(n²) intermediate copies:
each call copies all previously accumulated data.  Collect first, concat once.",
        bad_example: "\
result = pd.DataFrame()
for year in range(2000, 2020):
    result = pd.concat([result, df[df[\"year\"] == year]])  # O(n²) copies",
        good_example: "\
frames = [df[df[\"year\"] == year] for year in range(2000, 2020)]
result = pd.concat(frames)   # single allocation",
        url: Some("https://pandas.pydata.org/docs/reference/api/pandas.concat.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "NP003",
        name: "alloc-without-dtype",
        severity: "hint",
        domain: "numpy/pandas",
        rationale: "\
np.zeros, np.ones and np.empty default to float64 when dtype= is omitted.
On HPC systems processing integer data this silently doubles the memory
footprint and halves SIMD throughput.

np.full is the exception: it infers the dtype from the fill value, so
np.full(shape, 0) is int64 and np.full(shape, 0.0) is float64.  Being explicit
still matters — the inferred type follows a literal that is easy to change
without noticing the dtype change that follows it.",
        bad_example: "\
grid = np.zeros((1024, 1024))     # silently float64 — 8 MB per array
mask = np.ones((512, 512))        # same
fill = np.full((512, 512), 0)     # int64, inferred from the literal 0",
        good_example: "\
grid = np.zeros((1024, 1024), dtype=np.float32)
mask = np.ones((512, 512), dtype=np.int8)
fill = np.full((512, 512), 0, dtype=np.int16)",
        url: Some("https://numpy.org/doc/stable/reference/generated/numpy.zeros.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "NP004",
        name: "math-scalar-fn",
        severity: "warning",
        domain: "numpy/pandas",
        rationale: "\
Functions from Python's `math` module (sqrt, log, exp, etc.) operate on a
single scalar.  Inside a loop this means N Python function calls.  NumPy
ufuncs (np.sqrt, np.log) operate on whole arrays in C — the same work done
in a single call, with SIMD acceleration.

On a genuine scalar outside a loop, math.sqrt is *faster* than the numpy
ufunc, which pays array-dispatch overhead for one value.  Outside a loop the
rule therefore fires only when the argument is known to be an array.",
        bad_example: "\
for val in arr:
    output.append(math.sqrt(val))   # 10 000 Python calls for 10 000 elements",
        good_example: "\
output = np.sqrt(arr)   # single C call, vectorised",
        url: Some("https://numpy.org/doc/stable/reference/ufuncs.html"),
        fix_eligible: true,
    },
    ExplainEntry {
        id: "NP005",
        name: "chained-indexing",
        severity: "warning",
        domain: "numpy/pandas",
        rationale: "\
Chained indexing df[col][row] may return a copy of the data rather than a
view.  Assignments to the chained result silently do nothing — a common
source of hard-to-debug data corruption bugs.",
        bad_example: "\
df[\"a\"][5] = 99     # may write to a temporary copy; original unchanged",
        good_example: "\
df.loc[5, \"a\"] = 99   # guaranteed to modify the original DataFrame",
        url: Some(
            "https://pandas.pydata.org/docs/user_guide/indexing.html#returning-a-view-versus-a-copy",
        ),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "NP006",
        name: "matrix-deprecated",
        severity: "warning",
        domain: "numpy/pandas",
        rationale: "\
np.matrix was deprecated in NumPy 1.16 and is scheduled for removal.  It
has confusing semantics (elementwise * vs matrix multiply, always 2D) that
differ from ndarray.  All matrix operations are available on plain arrays
using the @ operator.",
        bad_example: "\
mat = np.matrix([[1, 2], [3, 4]])
result = mat * mat   # matrix multiply — confusing vs np.array",
        good_example: "\
arr = np.array([[1, 2], [3, 4]])
result = arr @ arr   # explicit matrix multiply with @",
        url: Some("https://numpy.org/doc/stable/reference/generated/numpy.matrix.html"),
        fix_eligible: true,
    },
    ExplainEntry {
        id: "NP007",
        name: "applymap-or-apply-lambda-in-loop",
        severity: "warning",
        domain: "numpy/pandas",
        rationale: "\
(a) DataFrame.applymap() was renamed to .map() in pandas 2.1 and will be
removed in a future release.
(b) .apply(lambda) inside a for loop applies a Python function element-by-
element on every iteration, creating an O(rows × iterations) Python overhead.",
        bad_example: "\
df_out = df.applymap(lambda x: x + 1)   # applymap deprecated
for col in cols:
    df[col].apply(lambda x: x * 2)       # loop + lambda = very slow",
        good_example: "\
df_out = df.map(lambda x: x + 1)        # use .map() instead
df[cols] = df[cols] * 2                  # vectorised — no lambda, no loop",
        url: Some("https://pandas.pydata.org/docs/reference/api/pandas.DataFrame.map.html"),
        fix_eligible: true,
    },
    // ── IO ────────────────────────────────────────────────────────────────────
    ExplainEntry {
        id: "IO001",
        name: "np-save-large-arrays",
        severity: "hint",
        domain: "io",
        rationale: "\
np.save stores arrays as raw uncompressed binary (.npy).  For large HPC
arrays this wastes disk space, cannot be read in parallel chunks, and
produces files that are hard to share across platforms.",
        bad_example: "\
np.save(\"wind_u.npy\", arr)   # uncompressed, unchunked, no metadata",
        good_example: "\
import zarr
from numcodecs import Blosc
zarr.save_array(\"wind_u.zarr\", arr, chunks=(256, 256),
                compressor=Blosc(cname=\"lz4\", clevel=5))",
        url: Some("https://zarr.readthedocs.io/en/stable/"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "IO002",
        name: "netcdf4-direct-open",
        severity: "hint",
        domain: "io",
        rationale: "\
netCDF4.Dataset bypasses xarray's coordinate alignment, CF metadata
handling, and lazy loading machinery.  Unless you need the low-level API
specifically, xr.open_dataset provides a safer and more feature-rich
alternative.",
        bad_example: "\
nc = netCDF4.Dataset(\"era5.nc\", \"r\")
u10 = nc.variables[\"u10\"][:]   # no lazy loading, no CF decode",
        good_example: "\
ds = xr.open_dataset(\"era5.nc\", chunks=\"auto\")
u10 = ds[\"u10\"]   # lazy, coordinate-aware",
        url: Some("https://docs.xarray.dev/en/stable/generated/xarray.open_dataset.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "IO003",
        name: "zarr-open-without-chunks",
        severity: "warning",
        domain: "io",
        rationale: "\
Opening a zarr store without chunks= stores the entire array as a single
chunk.  Single-chunk arrays cannot be compressed effectively, cannot be
read in parallel, and may not fit in memory.",
        bad_example: "\
store = zarr.open(\"wind.zarr\", mode=\"w\",
                  shape=(8760, 721, 1440), dtype=\"f4\")  # one giant chunk",
        good_example: "\
store = zarr.open(\"wind.zarr\", mode=\"w\",
                  shape=(8760, 721, 1440),
                  chunks=(24, 181, 360), dtype=\"f4\")",
        url: Some("https://zarr.readthedocs.io/en/stable/tutorial.html#chunk-optimizations"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "IO004",
        name: "netcdf4-read-in-loop",
        severity: "warning",
        domain: "io",
        rationale: "\
Each subscript access on a netCDF4 Variable may trigger a disk seek and read.
Inside a loop, N accesses mean N separate I/O operations — pre-loading the
full array outside the loop reduces this to a single read.",
        bad_example: "\
for i in range(12):
    monthly_means.append(temp[i].mean())   # 12 separate disk reads",
        good_example: "\
temp_data = nc.variables[\"temp\"][:]   # one read
monthly_means = [temp_data[i].mean() for i in range(12)]",
        url: None,
        fix_eligible: false,
    },
    ExplainEntry {
        id: "IO005",
        name: "h5py-file-without-swmr",
        severity: "hint",
        domain: "io",
        rationale: "\
HDF5 files opened without SWMR (Single Writer Multiple Reader) mode can
return stale or corrupt data when multiple MPI ranks or processes read the
same file concurrently.  SWMR mode uses atomic metadata updates to prevent
this.",
        bad_example: "\
f = h5py.File(\"data.h5\", \"r\")   # no SWMR — stale reads in parallel runs",
        good_example: "\
f = h5py.File(\"data.h5\", \"r\", swmr=True)   # safe for concurrent MPI readers",
        url: Some("https://docs.h5py.org/en/stable/swmr.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "IO006",
        name: "open-dataset-scipy-engine",
        severity: "warning",
        domain: "io",
        rationale: "\
xr.open_dataset with engine='scipy' reads the entire file eagerly into
memory using scipy.io.netcdf.  It does not support chunked/lazy access,
making it unsuitable for large HPC NetCDF files.  The netcdf4 or zarr
engines provide lazy, chunked loading.",
        bad_example: "\
ds = xr.open_dataset(\"large.nc\", chunks=\"auto\", engine=\"scipy\")
# chunks= is ignored — entire file still loaded eagerly",
        good_example: "\
ds = xr.open_dataset(\"large.nc\", chunks=\"auto\", engine=\"netcdf4\")",
        url: Some("https://docs.xarray.dev/en/stable/generated/xarray.open_dataset.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "XR012",
        name: "pathological-chunk-size",
        severity: "warning",
        domain: "xarray",
        rationale: "\
Array shapes need a runtime, so xray says nothing about whether a chunking is
well proportioned.  Chunk *arguments* are a different matter: they are literals
sitting in the source, and a chunk length of 1 is recognisable from the literal
alone as a mistake.

A chunk of 1 along a dimension means one dask task per index along that axis.
On a 40-year hourly dataset that is ~350,000 tasks, each carrying roughly a
millisecond of scheduler overhead and — on a parallel filesystem like Lustre —
its own metadata round-trip.  The scheduler and the MDS do all the work while
the compute nodes idle.

The rule also fires on a positional chunk spec whose literal extents multiply
out to fewer than 64 elements.  A *dict* spec is never judged that way: it
names the dimensions it constrains and leaves the rest at full extent, so
chunks={\"time\": 24} is a perfectly ordinary chunk, not a 24-element one.

Anything xray cannot read as a literal — chunks=\"auto\", a variable, -1
(which means \"one chunk along this axis\", a deliberate instruction) — is
left alone.",
        bad_example: "\
ds = xr.open_dataset(\"era5.nc\", chunks={\"time\": 1})   # one task per hour
ds = ds.chunk(time=1)                                  # same, method form
ds = xr.open_dataset(\"era5.nc\", chunks=(2, 4, 4))      # 32-element chunks",
        good_example: "\
ds = xr.open_dataset(\"era5.nc\", chunks={\"time\": 24 * 30})   # ~1 chunk/month
ds = xr.open_dataset(\"era5.nc\", chunks=\"auto\")              # let dask size it
ds = ds.chunk({\"time\": -1})                                 # deliberate: one chunk",
        url: Some(
            "https://docs.xarray.dev/en/stable/user-guide/dask.html#chunking-and-performance",
        ),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "DK010",
        name: "pathological-rechunk",
        severity: "warning",
        domain: "dask",
        rationale: "\
The same literal-only analysis as XR012, applied to the one dask call whose
entire purpose is choosing a chunk shape.

rechunk() is not free: it is a full shuffle, moving every block across the
graph.  Paying that to arrive at chunks of one element is the worst of both —
the shuffle cost up front, and a task-per-element graph afterwards.

As with XR012, only literals are judged.  A rechunk to a computed shape, to
\"auto\", or to -1 says nothing.",
        bad_example: "\
arr = arr.rechunk((1, 1000))   # a full shuffle, then one task per row
arr = arr.rechunk(1)           # one task per element, in every dimension",
        good_example: "\
arr = arr.rechunk((1000, 1000))   # ~8 MB chunks for float64
arr = arr.rechunk(\"auto\")         # let dask pick",
        url: Some("https://docs.dask.org/en/stable/generated/dask.array.rechunk.html"),
        fix_eligible: false,
    },
    // ── pandas ────────────────────────────────────────────────────────────────
    ExplainEntry {
        id: "PD001",
        name: "iterrows-in-loop",
        severity: "error",
        domain: "pandas",
        rationale: "\
NP001 flags iterrows() wherever it appears.  This is the worse case: the call
sits inside another loop, so the entire row-by-row Python pass is repeated on
every outer iteration.  A 10,000-row frame inside a 100-iteration loop is a
million Python-level row constructions, each one boxing every column value
into a Series.

Note what does *not* fire: a plain `for i, row in df.iterrows():` at the top
level.  The call is in the loop header, evaluated once, so it is NP001's
finding, not this one.",
        bad_example: "\
for group in groups:
    for i, row in df.iterrows():        # the full pass, once per group
        totals[group] += row[\"value\"]",
        good_example: "\
# One vectorised pass over the whole frame.
totals = df.groupby(\"group\")[\"value\"].sum()",
        url: Some("https://pandas.pydata.org/docs/user_guide/enhancingperf.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "PD002",
        name: "dataframe-append",
        severity: "error",
        domain: "pandas",
        rationale: "\
DataFrame.append() was deprecated in pandas 1.4 and *removed* in 2.0.  This is
not a warning you can live with: the call raises AttributeError on any current
pandas, so the script fails at the point it runs — often hours into a job.

It was also always a performance trap.  Each append allocated a whole new
frame and copied both operands, making an append loop O(n squared) in time and
memory.

Because `list.append` is the most common method call in Python, this rule
fires only when the receiver is a *known* pandas object — one xray watched
being assigned from `pd.read_csv`, `pd.DataFrame`, and so on.  An unknown
receiver is left alone, which is the opposite of xray's usual convention and
deliberately so.",
        bad_example: "\
out = pd.DataFrame()
for f in files:
    out = out.append(pd.read_csv(f))   # AttributeError on pandas >= 2.0",
        good_example: "\
frames = [pd.read_csv(f) for f in files]
out = pd.concat(frames, ignore_index=True)   # one allocation",
        url: Some("https://pandas.pydata.org/docs/whatsnew/v2.0.0.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "PD003",
        name: "chained-assignment",
        severity: "error",
        domain: "pandas",
        rationale: "\
`df[a][b] = value` is two operations.  The first, `df[a]`, may return a view
or a copy — pandas does not promise which — and the assignment then lands on
whatever that was.  When it is a copy, the write goes into a temporary that is
discarded on the next line, and the original frame is unchanged.

pandas flags this with SettingWithCopyWarning, which is easy to filter out or
miss in a job log.  Under copy-on-write, the default from pandas 3.0, it stops
warning and simply never writes.

NP005 flags the read form of the same chain.  This rule is the form that
silently loses data.

Nested indexing on a list of lists (`grid[1][2] = 0`) is not affected and is
not flagged: the rule requires evidence the receiver is a DataFrame, either a
string column key or a binding xray traced back to pandas.",
        bad_example: "\
df[\"temp\"][df[\"temp\"] < 0] = 0    # may write to a copy and vanish",
        good_example: "\
df.loc[df[\"temp\"] < 0, \"temp\"] = 0   # one indexer, always writes through",
        url: Some(
            "https://pandas.pydata.org/docs/user_guide/indexing.html#returning-a-view-versus-a-copy",
        ),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "PD004",
        name: "read-csv-without-dtype",
        severity: "hint",
        domain: "pandas",
        rationale: "\
Without dtype=, pandas infers each column's type by scanning the file, then
usually settles on float64 for numbers and object (a Python str per cell) for
anything else.  Two costs follow: the inference pass itself, which on a
multi-GB CSV can dominate the read, and a frame two to ten times larger in
memory than the data warrants.

Passing dtype= skips inference entirely and gives you the precision you meant.
Pairing it with usecols= is usually the bigger win again.

The rule stays quiet when the call forwards **kwargs, since dtype may well be
in there, and it does not fire for dask's or pyarrow's read_csv — their dtype
handling is a different story.",
        bad_example: "\
df = pd.read_csv(\"observations.csv\")   # infers over the whole file",
        good_example: "\
df = pd.read_csv(
    \"observations.csv\",
    usecols=[\"station\", \"temp\"],
    dtype={\"station\": \"category\", \"temp\": \"float32\"},
)",
        url: Some("https://pandas.pydata.org/docs/reference/api/pandas.read_csv.html"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "PD005",
        name: "to-csv-with-index",
        severity: "hint",
        domain: "pandas",
        rationale: "\
to_csv() writes the index by default, as a leading column with no header.
Whatever reads the file next sees a nameless first column and pandas names it
`Unnamed: 0`, so a round-trip through CSV silently grows a column each time.

For a default RangeIndex the column is pure noise.  For a meaningful index it
is real data — which is why an explicit `index=True` is treated as a decision
and left alone.  The rule only fires on the default.",
        bad_example: "\
df.to_csv(\"results.csv\")            # leading 0,1,2,... column
df2 = pd.read_csv(\"results.csv\")    # now has an `Unnamed: 0` column",
        good_example: "\
df.to_csv(\"results.csv\", index=False)
# or, when the index is real data you want to keep:
df.to_csv(\"results.csv\", index=True)",
        url: Some("https://pandas.pydata.org/docs/reference/api/pandas.DataFrame.to_csv.html"),
        fix_eligible: false,
    },
    // ── scipy ─────────────────────────────────────────────────────────────────
    ExplainEntry {
        id: "SP001",
        name: "quad-in-loop",
        severity: "warning",
        domain: "scipy",
        rationale: "\
scipy.integrate.quad is a scalar routine: one function, one interval, one
number out.  Calling it per element re-enters the Fortran QUADPACK driver
every iteration, re-allocating its workspace and re-running its adaptive error
control from scratch — and the Python callback is invoked once per evaluation
point, so the interpreter overhead is paid tens of times per call.

quad_vec integrates a vector-valued function in a single adaptive pass,
sharing one subdivision of the interval across all components.  When the
components have similar structure — the usual case for a parameter sweep —
this is not a constant-factor win but an asymptotic one, because the expensive
part (finding where the integrand is difficult) happens once.",
        bad_example: "\
results = []
for k in wavenumbers:
    val, err = quad(lambda x: f(x, k), 0, np.inf)   # full adaptive pass each time
    results.append(val)",
        good_example: "\
# One adaptive pass; the subdivision is shared across every k.
results, err = quad_vec(lambda x: f(x, wavenumbers), 0, np.inf)",
        url: Some(
            "https://docs.scipy.org/doc/scipy/reference/generated/scipy.integrate.quad_vec.html",
        ),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "SP002",
        name: "explicit-matrix-inverse",
        severity: "warning",
        domain: "scipy",
        rationale: "\
Forming an inverse to multiply by it is the textbook example of doing linear
algebra the expensive way.

inv(A) is an LU factorisation followed by n triangular solves, roughly 2n^3/3
+ 2n^3 flops; solve(A, b) is the same factorisation and a single solve, so
about half the work.  Accuracy is the more serious half of the argument: the
explicit inverse applies the condition number of A to the result twice, so on
an ill-conditioned system inv(A) @ b loses roughly twice as many digits as
solve(A, b) does.

If you need the same A many times, factor once with lu_factor / cho_factor and
reuse the factorisation — still not the inverse.

Genuine uses for an explicit inverse exist (a covariance matrix you must
report, for instance).  Suppress the rule on those lines; they are rare enough
to name individually.",
        bad_example: "\
x = scipy.linalg.inv(A) @ b                     # slower and less accurate
cov = scipy.linalg.inv(hessian) @ grad",
        good_example: "\
x = scipy.linalg.solve(A, b)

# Same A, many right-hand sides: factor once.
lu, piv = scipy.linalg.lu_factor(A)
xs = [scipy.linalg.lu_solve((lu, piv), b) for b in rhs]",
        url: Some("https://docs.scipy.org/doc/scipy/reference/generated/scipy.linalg.solve.html"),
        fix_eligible: false,
    },
    // ── HPC job scripts ───────────────────────────────────────────────────────
    ExplainEntry {
        id: "JOB001",
        name: "allocation-cluster-mismatch",
        severity: "warning",
        domain: "job",
        rationale: "\
Requires --job (or [job].script in xray.toml).

The scheduler gives the job the cores its directives asked for.  Dask uses the
cores the Python asked for.  Nothing checks that these are the same number,
and nothing complains at runtime if they are not — the job runs, produces
correct output, and leaves most of the allocation idle for the full wall time
while being billed for all of it.

xray compares n_workers x threads_per_worker against --cpus-per-task /
ncpus=.  Both sides must be readable literals: a shell variable in the
directive or a computed worker count produces no finding rather than a guess.",
        bad_example: "\
#SBATCH --cpus-per-task=48
...
cluster = LocalCluster(n_workers=4)   # 44 of 48 cores idle, billed anyway",
        good_example: "\
#SBATCH --cpus-per-task=48
...
import os
n = int(os.environ[\"SLURM_CPUS_PER_TASK\"])
cluster = LocalCluster(n_workers=n, threads_per_worker=1)",
        url: Some("https://github.com/greensh16/xray-cs/wiki/Job-Rules#job001"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "JOB002",
        name: "unpinned-thread-count",
        severity: "warning",
        domain: "job",
        rationale: "\
Requires --job (or [job].script in xray.toml).

NumPy's BLAS backend (OpenBLAS, MKL) defaults to one thread per core it can
see — and on a shared node it sees the whole node, not your allocation.  Every
dask worker then starts its own BLAS pool, so N workers on a 48-core node can
request 48 threads each.  The result is thousands of runnable threads
thrashing the scheduler, and the classic HPC surprise: the parallel job runs
slower than the serial one.

Either lever silences the rule, because either one fixes it: exporting
OMP_NUM_THREADS / MKL_NUM_THREADS in the job script, or passing
threads_per_worker= in Python.  xray does not audit the number, only that a
decision was made.",
        bad_example: "\
#SBATCH --cpus-per-task=48
# nothing pins the BLAS pool
cluster = LocalCluster(n_workers=48)   # up to 48 x 48 threads",
        good_example: "\
#SBATCH --cpus-per-task=48
export OMP_NUM_THREADS=1
export MKL_NUM_THREADS=1
cluster = LocalCluster(n_workers=48, threads_per_worker=1)",
        url: Some("https://github.com/greensh16/xray-cs/wiki/Job-Rules#job002"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "JOB003",
        name: "memory-request-unchunked-read",
        severity: "error",
        domain: "job",
        rationale: "\
Requires --job (or [job].script in xray.toml).

XR001 already says an unchunked open_dataset loads eagerly.  This rule says
something sharper: the job has a hard memory ceiling, written down two files
away, and an eager read either fits under it or the scheduler kills the job.
There is no graceful degradation, and the failure arrives after the queue wait
rather than at submission.

Reported as an error because it is the one JOB rule whose outcome is a dead
job rather than a wasted allocation.",
        bad_example: "\
#SBATCH --mem=190GB
...
ds = xr.open_mfdataset(\"era5_*.nc\")   # eager; OOM-killed if it exceeds 190 GB",
        good_example: "\
#SBATCH --mem=190GB
...
ds = xr.open_mfdataset(\"era5_*.nc\", chunks={\"time\": 24})   # streams lazily",
        url: Some("https://github.com/greensh16/xray-cs/wiki/Job-Rules#job003"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "JOB004",
        name: "unused-gpu-allocation",
        severity: "warning",
        domain: "job",
        rationale: "\
Requires --job (or [job].script in xray.toml).

A GPU node costs several times a CPU node per hour and is usually the
scarcest queue on the machine.  A job that requests one and never imports a
library able to use it burns that budget and holds the device away from
whoever needed it — and nothing in the Python or the job script complains,
because neither half is wrong on its own.

This is the only rule in xray evaluated across the whole run rather than
per file: the question is whether *anything* the job launches touches the GPU,
so a package where model.py imports torch and utils.py does not is fine.  It
reports once, against the offending directive in the job script.

The GPU-library list is deliberately generous (cupy, torch, tensorflow, jax,
cudf, ...) — torch has a CPU-only build, but a script importing torch and
requesting a GPU is not the mistake this rule is looking for.",
        bad_example: "\
#SBATCH --gres=gpu:v100:2     # <- reported here
...
import numpy as np            # nothing that can reach the device",
        good_example: "\
#SBATCH --cpus-per-task=12    # no GPU asked for
...
import numpy as np",
        url: Some("https://github.com/greensh16/xray-cs/wiki/Job-Rules#job004"),
        fix_eligible: false,
    },
    ExplainEntry {
        id: "JOB005",
        name: "unbounded-worker-pool",
        severity: "warning",
        domain: "job",
        rationale: "\
Requires --job (or [job].script in xray.toml).

n_jobs=-1 means \"every core\", and multiprocessing.Pool() with no argument
means the same.  Both read the *machine's* core count, not the job's
allocation: cgroups constrain the CPU time those processes get but do not
change what os.cpu_count() reports.  On a shared node a 4-core job therefore
spawns 48 workers, which oversubscribes your own allocation and degrades every
other job on the node.

The rule fires only under a partial-node allocation.  With --exclusive the job
owns the machine, and taking every core on it is exactly right.",
        bad_example: "\
#SBATCH --cpus-per-task=4
...
Parallel(n_jobs=-1)(delayed(work)(x) for x in items)   # 48 workers on 4 cores",
        good_example: "\
#SBATCH --cpus-per-task=4
...
import os
n = int(os.environ.get(\"SLURM_CPUS_PER_TASK\", 1))
Parallel(n_jobs=n)(delayed(work)(x) for x in items)",
        url: Some("https://github.com/greensh16/xray-cs/wiki/Job-Rules#job005"),
        fix_eligible: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_has_an_explain_entry() {
        // `xray explain <ID>` must work for every rule --list-rules shows.
        for meta in crate::rules::all_meta() {
            assert!(
                ENTRIES.iter().any(|e| e.id == meta.id),
                "no ExplainEntry for {}",
                meta.id
            );
        }
    }

    #[test]
    fn no_explain_entry_is_orphaned() {
        let known: Vec<&str> = crate::rules::all_meta().iter().map(|m| m.id).collect();
        for e in ENTRIES {
            assert!(known.contains(&e.id), "ExplainEntry {} has no rule", e.id);
        }
    }

    #[test]
    fn unknown_ids_return_false_so_main_exits_two() {
        // `explain` prints the error itself; main must not print a second one.
        assert!(!explain("NOPE"));
        assert!(explain("XR001"));
    }

    #[test]
    fn rule_ids_are_case_insensitive() {
        assert!(explain("xr001"));
    }
}
