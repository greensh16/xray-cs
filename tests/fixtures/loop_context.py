"""
Regression fixture for loop-context detection: comprehensions and while loops
are loops too.  Every construct here SHOULD be flagged.
"""
import xarray as xr
import dask


def in_comprehension(items):
    # DK001 — .compute() once per element
    return [x.compute() for x in items]


def in_while_loop(ds):
    i = 0
    while i < 10:
        # DK001 — .compute() once per iteration
        ds.compute()
        i += 1


def compute_in_loop_header(ds):
    # NOT a per-iteration compute: the iterable is evaluated once, so no
    # compute-in-loop rule should fire here.
    for row in ds.mean().compute():  # xray: disable=DK004
        print(row)


def negative_float_coord(ds):
    # XR004 — negative latitude is still a float coordinate
    return ds.sel(lat=-33.5)


_ = dask
_ = xr
