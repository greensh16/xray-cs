import dask.array as da
import dask
import numpy as np

# DK001: .compute() inside a for loop
results = []
for i in range(10):
    chunk = da.from_array(np.ones((100, 100)), chunks=50)
    result = chunk.mean().compute()   # full graph rebuild every iteration
    results.append(result)

# DK002: dask.compute() inside a for loop
delayed_items = [dask.delayed(lambda x: x * 2)(i) for i in range(5)]
for item in delayed_items:
    val = dask.compute(item)   # should batch-compute outside the loop

# DK003: excessive .compute() calls (fires when count >= threshold in config)
a = da.ones((1000, 1000), chunks=100)
b = da.zeros((1000, 1000), chunks=100)
r1 = a.sum().compute()
r2 = b.mean().compute()
r3 = (a + b).std().compute()

# DK004: immediate .compute() — no lazy benefit
instant = da.from_array(np.arange(1000), chunks=100).compute()

# DK005: .persist() result discarded — pointless warming of the graph
a.persist()

# DK006: .persist().compute() — persist then immediately compute defeats lazy evaluation
instant2 = a.persist().compute()

# DK007: da.from_array() without chunks= — single monolithic chunk, no parallelism
big_array = da.from_array(np.random.rand(10000, 10000))

# This is fine — chunks specified
chunked = da.from_array(np.random.rand(10000, 10000), chunks=(1000, 1000))

# DK008: .rechunk() inside a for loop — full re-partition every iteration
for step in range(5):
    a = a.rechunk({0: 200})
    r = a.sum().compute()

# DK009: da.concatenate() inside a for loop — O(n²) intermediate copies
acc = da.zeros((0,), chunks=100)
for i in range(10):
    acc = da.concatenate([acc, da.ones((100,), chunks=100)])

# DK010 — a full shuffle to arrive at one task per row.
grid = da.ones((10000, 1000), chunks=1000)
tiny = grid.rechunk((1, 1000))
