import numpy as np
import pandas as pd
import math

df = pd.DataFrame({"a": range(1000), "b": range(1000, 2000)})
arr = np.arange(10000, dtype=float)

# NP001: iterrows — row-by-row python loop
totals = []
for idx, row in df.iterrows():
    totals.append(row["a"] + row["b"])

# NP002: pd.concat inside a for loop — quadratic copies
frames = []
result = pd.DataFrame()
for year in range(2000, 2020):
    chunk = df[df["a"] > year]
    result = pd.concat([result, chunk])   # should collect then concat once

# NP002: np.concatenate inside a loop
combined = np.array([])
for i in range(20):
    combined = np.concatenate([combined, arr[i * 100 : (i + 1) * 100]])

# NP003: np.zeros without dtype
grid = np.zeros((1024, 1024))           # silently float64
mask = np.ones((512, 512))             # silently float64

# These are fine
grid_ok = np.zeros((1024, 1024), dtype=np.float32)
mask_ok = np.ones((512, 512), dtype=np.int8)

# NP004: math.* inside a loop — should use ufunc
output = []
for val in arr:
    output.append(math.sqrt(val))       # replace with np.sqrt(arr)

logs = []
for val in arr:
    logs.append(math.log(val + 1))     # replace with np.log(arr + 1)

# NP005: chained indexing
val = df["a"][0]                        # copy semantics, assignment won't propagate
df["a"][5] = 99                         # silently writes to a copy, not the DataFrame

# NP004: math.* applied to an array outside a loop — hint level.
# math.sqrt on a whole array is the case worth flagging: np.sqrt vectorises it.
array_sqrt = math.sqrt(arr)            # replace with np.sqrt(arr)

# NP006: np.matrix() is deprecated since NumPy 1.16
mat = np.matrix([[1, 2], [3, 4]])

# NP007a: DataFrame.applymap() deprecated since pandas 2.1 — use .map() instead
df_mapped = df.applymap(lambda x: x + 1)

# NP007b: .apply(lambda) inside a for loop — extremely slow row-by-row execution
for col in ["a", "b"]:
    df[col].apply(lambda x: x * 2)   # expression statement — matches pattern
