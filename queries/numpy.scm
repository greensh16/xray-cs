; NP001 — DataFrame.iterrows() — notorious O(n) row-by-row iteration.
; Use vectorised operations or df.apply() instead.
(call
  function: (attribute
    attribute: (identifier) @np_iterrows
    (#eq? @np_iterrows "iterrows")
  )
) @np_iterrows_call


; NP002 — pd.concat / np.concatenate call.  Rust uses is_inside_for_loop()
; to restrict to loop bodies — quadratic copy overhead.
(call
  function: [
    (attribute
      attribute: (identifier) @np_concat_method
      (#match? @np_concat_method "^(concat|concatenate)$")
    )
    (identifier) @np_concat_bare
    (#match? @np_concat_bare "^(concat|concatenate)$")
  ]
) @np_concat_call


; NP003 — np.zeros / np.ones / np.empty called without an explicit dtype=.
; Defaults to float64 — on HPC this silently doubles memory for int workloads.
(call
  function: [
    (attribute
      attribute: (identifier) @np_alloc_method
      (#match? @np_alloc_method "^(zeros|ones|empty|full)$")
    )
    (identifier) @np_alloc_bare
    (#match? @np_alloc_bare "^(zeros|ones|empty|full)$")
  ]
  arguments: (argument_list) @np_alloc_args
) @np_alloc_call


; NP004 — math.* scalar function called anywhere.
; Severity is determined in Rust: Warning when inside a for loop,
; Hint otherwise. Matches all call sites so the Rust code can
; walk the ancestor chain to determine loop context.
(call
  function: (attribute
    object: (identifier) @np_math_mod
    attribute: (identifier) @np_math_fn
    (#eq? @np_math_mod "math")
    (#match? @np_math_fn "^(sqrt|log|log2|log10|exp|sin|cos|tan)$")
  )
) @np_math_call


; NP005 — Chained indexing df[col][row] — creates a copy, assignments silently
; don't propagate back to the original DataFrame.
(subscript
  value: (subscript) @np_inner_sub
) @np_chained_index


; NP006 — np.matrix() is deprecated since NumPy 1.16 and will be removed.
; np.matrix has surprising elementwise vs matrix multiplication semantics;
; use np.ndarray (np.array) for all new code.
(call
  function: [
    (attribute
      attribute: (identifier) @np_matrix_attr
      (#eq? @np_matrix_attr "matrix")
    )
    (identifier) @np_matrix_bare
    (#eq? @np_matrix_bare "matrix")
  ]
) @np_matrix_call


; NP007a — DataFrame.applymap() is deprecated since pandas 2.1; use .map() instead.
(call
  function: (attribute
    attribute: (identifier) @np_applymap_attr
    (#eq? @np_applymap_attr "applymap")
  )
) @np_applymap_call


; NP007b — DataFrame/Series .apply(lambda ...) inside a loop.
; Applying a Python lambda row-by-row or element-by-element inside a loop
; is extremely slow — vectorise the operation or use .map() / .apply() once.
;
; Loop context is decided in Rust via `is_inside_loop`, as with every other
; loop-sensitive rule. The previous `(_)* ... (_)*` shape could match at
; several split points (emitting duplicate diagnostics for one call) and only
; saw direct `expression_statement` children of the body, so an assigned
; result — `x = df.apply(lambda ...)` — was missed entirely.
(call
  function: (attribute
    attribute: (identifier) @np_apply_method
    (#eq? @np_apply_method "apply")
  )
  arguments: (argument_list
    (lambda) @np_apply_lambda
  )
) @np_apply_in_loop
