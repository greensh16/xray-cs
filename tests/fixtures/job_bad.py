"""
Python half of the JOB fixture — see job_bad.sh for the allocation it is
cross-checked against.
"""
import xarray as xr
from dask.distributed import LocalCluster
from joblib import Parallel, delayed

# JOB001 — 4 workers under a 48-core allocation.
# JOB002 — nothing pins the BLAS thread pool, here or in the job script.
cluster = LocalCluster(n_workers=4)

# JOB003 — eager read under a hard 190 GB ceiling.
ds = xr.open_mfdataset("era5_*.nc")

# JOB005 — every core on the node, not every core of the allocation.
Parallel(n_jobs=-1)(delayed(print)(i) for i in range(4))
