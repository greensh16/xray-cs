"""
Fixture for the pandas domain (PD001–PD005).
Every rule in the domain should fire at least once here.
"""
import pandas as pd

df = pd.read_csv("observations.csv")            # PD004 — no dtype=
frames = pd.read_csv("more.csv")                # PD004

# PD001 — the full row-by-row pass, once per outer iteration.
totals = {}
for group in ["a", "b", "c"]:
    for i, row in df.iterrows():
        totals[group] = row["value"]

# PD002 — removed in pandas 2.0; raises AttributeError.
out = pd.DataFrame()
for f in ["x.csv", "y.csv"]:
    out = out.append(pd.read_csv(f, dtype="float32"))

# PD003 — the write lands on a temporary copy.
df["temp"]["0"] = 0

# PD005 — writes a nameless index column.
df.to_csv("results.csv")
