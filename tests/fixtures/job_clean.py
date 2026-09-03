"""
Python half of the clean JOB fixture — must produce no JOB diagnostics when
cross-checked against job_clean.sh.
"""
import os

import xarray as xr
from dask.distributed import LocalCluster
from joblib import Parallel, delayed

n = int(os.environ["SLURM_CPUS_PER_TASK"])

# JOB001 OK: 48 x 1 matches --cpus-per-task=48.
# JOB002 OK: threads_per_worker is set, and the job script exports the caps.
cluster = LocalCluster(n_workers=48, threads_per_worker=1)

# JOB003 OK: chunked, so the read streams within the memory ceiling.
ds = xr.open_mfdataset("era5_*.nc", chunks={"time": 24}, parallel=True)

# JOB005 OK: the pool is sized from the allocation.
Parallel(n_jobs=n)(delayed(print)(i) for i in range(4))
