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
        fix_eligible: true,
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
        fix_eligible: true,
    },
];
