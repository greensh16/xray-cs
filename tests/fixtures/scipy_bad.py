"""
Fixture for the scipy domain (SP001–SP002).
"""
import numpy as np
import scipy
from scipy.integrate import quad
from scipy.linalg import inv

# SP001 — a full adaptive quadrature pass per wavenumber.
results = []
for k in range(10):
    val, err = quad(lambda x: np.exp(-x * k), 0, 1)
    results.append(val)

# SP001 — the module-attribute spelling resolves too.
for k in range(10):
    scipy.integrate.quad(lambda x: x, 0, k)

# SP002 — explicit inverse instead of a solve.
A = np.eye(4)
b = np.ones(4)
x = inv(A) @ b
y = scipy.linalg.inv(A) @ b
