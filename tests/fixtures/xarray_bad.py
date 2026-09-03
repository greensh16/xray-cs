import xarray as xr
import numpy as np

# XR001: open_dataset without chunks
ds = xr.open_dataset("era5.nc")

# XR001: open_mfdataset without chunks
ds_multi = xr.open_mfdataset("era5_*.nc")

# This is fine — has chunks
ds_ok = xr.open_dataset("era5.nc", chunks={"time": 10})

# XR002: .values access strips coordinates
arr = ds["u10"].values

# XR003: loop over a dimension attribute
for t in ds.time:
    print(t)

# XR004: .sel() with a float literal
point = ds.sel(lat=45.0, lon=-120.5)

# XR005: .compute() inside a for loop
for year in range(2000, 2024):
    subset = ds.sel(time=str(year))
    result = subset.compute()  # re-triggers full graph every iteration

# XR006: to_array() / to_dataarray() without dim= — unnamed concat dimension
arr_stacked = ds.to_array()
arr_stacked2 = ds.to_dataarray()

# XR007: xr.concat inside a for loop — O(n²) intermediate copies
combined = ds.isel(time=0)
for i in range(1, 10):
    combined = xr.concat([combined, ds.isel(time=i)], dim="time")

# XR008: open_mfdataset without parallel=True — serial file opening
ds_slow = xr.open_mfdataset("era5_*.nc", chunks={"time": 10})

# This is fine — parallel=True
ds_fast = xr.open_mfdataset("era5_*.nc", chunks={"time": 10}, parallel=True)

# XR009: apply_ufunc with dask="allowed" — silently falls back to serial
result_ufunc = xr.apply_ufunc(np.exp, ds["u10"], dask="allowed")

# This is fine — dask="parallelized"
result_ufunc_ok = xr.apply_ufunc(np.exp, ds["u10"], dask="parallelized", output_dtypes=[float])

# XR010: xr.merge inside a for loop — O(n²) alignment cost
merged = xr.Dataset()
for year in range(2000, 2010):
    merged = xr.merge([merged, ds.sel(time=str(year))])

# XR011: to_netcdf without encoding= — no compression, full float64
ds.to_netcdf("output.nc")

# This is fine — encoding specified
ds.to_netcdf("output_compressed.nc", encoding={"u10": {"dtype": "float32", "zlib": True}})

# XR012 — a chunk length of 1: one task, and one Lustre metadata round-trip,
# per index along `time`.
ds_singleton = xr.open_dataset("era5.nc", chunks={"time": 1, "lat": 181})
ds_rechunked = ds_singleton.chunk(time=1)
