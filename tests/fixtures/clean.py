"""
Clean reference script — xray should emit zero diagnostics.
This is what well-written HPC scientific Python looks like.
"""
import xarray as xr
import numpy as np
import dask.array as da
import zarr
from numcodecs import Blosc

# XR001 OK: chunks= provided
ds = xr.open_dataset("era5.nc", chunks={"time": 24, "lat": 181, "lon": 360})
ds_multi = xr.open_mfdataset("era5_*.nc", chunks={"time": 24}, parallel=True)

# XR002 OK: .to_numpy() is explicit, .data keeps dask backing
arr_np = ds["u10"].to_numpy()
arr_lazy = ds["u10"].data

# XR003 OK: vectorised indexing instead of Python loop
n_times = ds.sizes["time"]
subset = ds.isel(time=slice(0, n_times // 2))

# XR004 OK: method='nearest' provided
point = ds.sel(lat=45.0, lon=-120.5, method="nearest")

# XR005 OK: .persist() before loop, then index inside
ds_hot = ds.persist()
results = []
for year in range(2000, 2024):
    results.append(ds_hot.sel(time=str(year)).mean().compute())  # xray: disable=DK001,XR005

# DK003 OK: single compute on the final combined result
a = da.ones((1000, 1000), chunks=100)
b = da.zeros((1000, 1000), chunks=100)
combined = (a + b).mean().compute()

# NP001 OK: vectorised operation
import pandas as pd
df = pd.DataFrame({"a": range(1000), "b": range(1000, 2000)})
totals = df["a"] + df["b"]

# NP002 OK: collect then concat once
frames = [df[df["a"] > year] for year in range(2000, 2020)]
result = pd.concat(frames)

# NP002 OK: preallocate
chunks = [arr_np[i * 100 : (i + 1) * 100] for i in range(10)]
combined_np = np.concatenate(chunks)

# NP003 OK: explicit dtype
grid = np.zeros((1024, 1024), dtype=np.float32)
mask = np.ones((512, 512), dtype=np.int8)

# NP004 OK: ufunc over the whole array
arr = np.arange(1.0, 10001.0)
output = np.sqrt(arr)
logs = np.log(arr)

# NP005 OK: .loc for safe indexing
val = df.loc[0, "a"]
df.loc[5, "a"] = 99

# IO001 OK: Zarr with compression
compressor = Blosc(cname="lz4", clevel=5)
z = zarr.open(
    "wind.zarr",
    mode="w",
    shape=(8760, 721, 1440),
    chunks=(24, 181, 360),
    dtype="f4",
    compressor=compressor,
)

# IO002 OK: xarray instead of netCDF4 direct
ds_nc = xr.open_dataset("era5.nc", chunks="auto")

# IO004 OK: pre-load variable outside loop
import netCDF4
nc = netCDF4.Dataset("barra2.nc", "r")  # xray: disable=IO002
temp_data = nc.variables["temp"][:]    # load once  # xray: disable=NP005
nc.close()
monthly_means = [temp_data[i].mean() for i in range(12)]

# XR006 OK: to_array() with explicit dim= provided
import xarray as xr
ds2 = xr.open_dataset("era5.nc", chunks={"time": 24})
arr_named = ds2.to_array(dim="variable")
arr_named2 = ds2.to_dataarray(dim="variable")

# XR007 OK: collect slices in a list, call xr.concat once outside the loop
slices = [ds2.isel(time=i) for i in range(10)]
combined = xr.concat(slices, dim="time")

# DK005 OK: assign persist() result so the warmed graph is reused
import dask.array as da
a_lazy = da.ones((1000, 1000), chunks=100)
a_hot = a_lazy.persist()  # result captured — graph is in memory for reuse across ops
x = a_hot.mean()  # lazy op on the persisted result
y = a_hot.sum()   # another lazy op — no extra persist needed

# DK006 OK: use .compute() directly without a preceding .persist() in the same chain
# or use .persist() and compute the persisted reference rather than chaining
b_hot = a_lazy.persist()
b_mean = b_hot.mean()   # keep lazy until final step

# NP006 OK: use np.ndarray instead of deprecated np.matrix
arr2d = np.array([[1, 2], [3, 4]])
mat_result = arr2d @ arr2d    # use @ operator for matrix multiply

# NP007a OK: use .map() (pandas 2.1+) instead of deprecated .applymap()
import pandas as pd
df2 = pd.DataFrame({"a": range(10), "b": range(10, 20)})
df_mapped_ok = df2.map(lambda x: x + 1)

# NP007b OK: vectorise with .apply() once outside any loop
df2["a"] = df2["a"].apply(lambda x: x * 2)

# IO005 OK: h5py.File with swmr=True for safe parallel reads
import h5py
f_safe = h5py.File("data.h5", "r", swmr=True)

# IO006 OK: use engine="netcdf4" or engine="zarr" for lazy HPC-friendly reads
ds_nc4 = xr.open_dataset("data.nc", chunks="auto", engine="netcdf4")
