//! Synthetic corpus generator shared by the benchmarks and the performance
//! guard test.
//!
//! Real-world corpora cannot be committed and vary between machines, so the
//! performance numbers are measured against generated code with a known shape:
//! a fixed mix of clean lines and rule-triggering lines, so throughput is
//! comparable across runs and across versions.
//!
//! The mix matters. A corpus of nothing but violations measures the diagnostic
//! construction path; a corpus of nothing but clean code measures the query
//! path and never allocates a `Diagnostic`. Both are unrepresentative, so
//! [`module`] emits roughly one finding per 12 lines — noisier than healthy
//! code, quiet enough that diagnostic building does not dominate.

/// One synthetic Python module, roughly 45 lines.
///
/// Touches every rule domain so that per-domain benchmarks all have work to
/// do: xarray opens and chunk specs, dask computes, numpy allocation and
/// iteration, pandas frames, scipy quadrature, and netCDF/zarr I/O.
pub fn module(i: usize) -> String {
    format!(
        r#"import xarray as xr
import numpy as np
import pandas as pd
import dask.array as da
import scipy.integrate as integrate
import zarr

CHUNKS_{i} = {{"time": 24, "lat": 181}}


def load_{i}(path):
    """Clean: chunked read, explicit dtype."""
    ds = xr.open_dataset(path, chunks=CHUNKS_{i})
    buf = np.zeros((256, 256), dtype=np.float32)
    return ds, buf


def summarise_{i}(ds):
    ds_eager = xr.open_dataset("other.nc")
    totals = []
    for t in range(24):
        totals.append(ds.temp.isel(time=t).mean().compute())
    return ds_eager, totals


def tabulate_{i}(path):
    df = pd.read_csv(path)
    out = np.zeros((128, 128))
    for idx, row in df.iterrows():
        out[idx % 128] = row["value"]
    df.to_csv("out_{i}.csv")
    return out


def integrate_{i}(ks):
    results = []
    for k in ks:
        value, _err = integrate.quad(lambda x: x * k, 0, 1)
        results.append(value)
    return results


def store_{i}(arr):
    lazy = da.from_array(arr)
    tiny = lazy.rechunk((1, 512))
    z = zarr.open("out_{i}.zarr", mode="w", shape=(64, 64), dtype="f4")
    z[:] = arr[:64, :64]
    return tiny


def clean_{i}(ds):
    """Deliberately quiet: exercises the query path with no findings."""
    mean = ds.temp.mean(dim="time")
    scaled = mean * 2.0
    named = scaled.rename("temp_mean")
    return named.to_dataset()
"#
    )
}

/// `n` synthetic modules, as `(name, source)` pairs.
pub fn corpus(n: usize) -> Vec<(String, String)> {
    (0..n).map(|i| (format!("mod_{i}.py"), module(i))).collect()
}

/// Total line count of a corpus, for throughput reporting.
pub fn total_lines(corpus: &[(String, String)]) -> u64 {
    corpus.iter().map(|(_, s)| s.lines().count() as u64).sum()
}

/// Number of modules needed to reach approximately `target` lines.
pub fn modules_for_lines(target: usize) -> usize {
    let per_module = module(0).lines().count().max(1);
    target.div_ceil(per_module)
}
