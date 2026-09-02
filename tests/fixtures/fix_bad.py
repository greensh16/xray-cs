"""Every auto-fixable rule, exactly once. Paired with fix_fixed.py."""
import xarray as xr
import dask.array as da
import pandas as pd
import numpy as np
import math

vals = [1, 2, 3]
arr = np.arange(4, dtype=np.float32)

a = xr.open_dataset("a.nc")
b = xr.open_mfdataset("*.nc", chunks="auto")
c = xr.apply_ufunc(len, a, dask='allowed')
d = da.from_array(vals)
e = np.matrix([[1, 2]])
g = math.sqrt(arr)
h = pd.DataFrame({"a": [1]}).applymap(str)
