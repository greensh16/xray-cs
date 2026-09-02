"""
Regression fixture: every construct here is legitimate code that xray used to
flag.  It must produce ZERO diagnostics.

Each block names the rule that used to misfire.
"""
import xarray as xr
import pandas as pd
import numpy as np
import netCDF4
import zarr
import h5py
import dask.array as da
import math


# XR003 — iterating an ordinary attribute is not a dimension loop
class Loader:
    def __init__(self):
        self.files = ["a.nc", "b.nc"]

    def run(self):
        for f in self.files:
            print(f)


# XR007 / XR010 — pandas concat/merge are not xarray calls.
# (Doing either inside a loop is a real NP002 finding, so the loop-free form
# is used here; the "in a loop" case is asserted separately by rule ID.)
def combine(frames, dfs):
    merged = frames[0].merge(dfs[0], on="id")
    return pd.concat([merged, *dfs])


# IO003 — the builtin open() is not zarr.open()
def read_notes(path):
    with open(path) as fh:
        return fh.read()


# IO004 — ordinary list/dict indexing inside a loop is not a netCDF4 read
def totals(items, lookup):
    acc = []
    for i in range(10):
        acc.append(items[i])
        print(lookup["key"])
    return acc


# NP005 — nested indexing on a list of lists is not pandas chained indexing
def corner(grid):
    return grid[0][1]


# XR001 / DK007 — the keyword may be supplied through **kwargs
OPTS = {"chunks": "auto"}


def open_with_options(path):
    return xr.open_dataset(path, **OPTS)


def from_array_with_options(arr, **kwargs):
    return da.from_array(arr, **kwargs)


# NP002 — concatenating outside a loop is the recommended form
def stack(parts):
    return np.concatenate(parts)


# IO005 — h5py.File with swmr= supplied
def open_h5(path):
    return h5py.File(path, "r", swmr=True)


# IO002 is config-gated, netCDF4 imported only to arm the IO domain above
_ = netCDF4
_ = zarr


# NP004 must not fire on genuine scalars outside a loop: for a single float,
# math.sqrt is faster than the numpy ufunc, so the hint was actively wrong.
scalar_root = math.sqrt(2.0)
scalar_log = math.log(10.0)

# XR002 must not fire on a pandas receiver: `.values` is the documented idiom
# on a DataFrame, and only looks like the xarray anti-pattern.
frame = pd.DataFrame({"a": [1, 2, 3]})
frame_values = frame.values

# XR002 must not fire on a plain numpy array either.
plain = np.zeros(4, dtype=np.float32)
plain_values = plain.values

# DK004 must not fire on the idiomatic reduce-then-compute: dask did the
# parallel work and the small result is what you asked for.
ds_lazy = xr.open_dataset("big.nc", chunks="auto")
reduced = ds_lazy.mean().compute()
total = ds_lazy.sum().compute()
