; DK001 — .compute() method call.  Rust uses is_inside_for_loop() to restrict
; this to loop bodies — .persist() before the loop is the recommended fix.
(call
  function: (attribute
    attribute: (identifier) @dk_compute_method
    (#eq? @dk_compute_method "compute")
  )
) @dk_compute_call


; DK002 — dask.compute() module-level call.  Rust checks loop context.
(call
  function: (attribute
    object: (identifier) @dk_mod
    attribute: (identifier) @dk_compute_fn
    (#eq? @dk_mod "dask")
    (#eq? @dk_compute_fn "compute")
  )
) @dk_dask_compute_call


; DK003 — Any .compute() call at all (collected for XR005 / duplicate-compute
; analysis in the Rust visitor — rule fires only when count > threshold).
(call
  function: (attribute
    attribute: (identifier) @dk_any_compute
    (#eq? @dk_any_compute "compute")
  )
) @dk_any_compute_call


; DK004 — .compute() called immediately on the result of another call with no
; intermediate assignment.  Pattern: some_dask_call(...).compute()
; This means the lazy graph is never reused — pointless dask overhead.
; Excludes the .persist().compute() chain (handled by DK006).
(call
  function: (attribute
    object: (call
      function: (attribute
        attribute: (identifier) @dk_inner_method
        (#not-eq? @dk_inner_method "persist")
      )
    ) @dk_inner_call
    attribute: (identifier) @dk_immediate_compute
    (#eq? @dk_immediate_compute "compute")
  )
) @dk_immediate_compute_call


; DK005 — .persist() result not captured (expression statement, result discarded).
; persist() materialises the graph in distributed memory — if the result is
; discarded immediately the full cost is paid with no benefit.
(expression_statement
  (call
    function: (attribute
      attribute: (identifier) @dk_persist_discarded
      (#eq? @dk_persist_discarded "persist")
    )
  ) @dk_persist_uncaptured
)


; DK006 — .persist() immediately chained with .compute().
; persist() is for reusing a result across multiple operations. Chaining
; .persist().compute() in one expression negates the benefit — use .compute() alone.
(call
  function: (attribute
    object: (call
      function: (attribute
        attribute: (identifier) @dk_chained_persist
        (#eq? @dk_chained_persist "persist")
      )
    )
    attribute: (identifier) @dk_chained_compute
    (#eq? @dk_chained_compute "compute")
  )
) @dk_persist_then_compute


; DK007 — da.from_array() without chunks= keyword argument.
; from_array without chunks= creates a single-chunk array (the whole array in
; one partition), defeating the purpose of Dask — the graph runs serially and
; may OOM on large arrays. Always specify chunks= explicitly.
(call
  function: [
    (attribute
      attribute: (identifier) @dk_from_array_attr
      (#eq? @dk_from_array_attr "from_array")
    )
    (identifier) @dk_from_array_bare
    (#eq? @dk_from_array_bare "from_array")
  ]
) @dk_from_array_call


; DK008 — .rechunk() call inside a loop.
; rechunk() triggers a full graph materialisation and re-partition on each
; iteration — O(n) rechunks are almost always a design smell. Rechunk once
; before the loop with the target chunk shape.
(call
  function: (attribute
    attribute: (identifier) @dk_rechunk_method
    (#eq? @dk_rechunk_method "rechunk")
  )
) @dk_rechunk_call


; DK009 — da.concatenate() / dask.array.concatenate() inside a loop.
; Same O(n²) accumulation pattern as np.concatenate or xr.concat in a loop:
; each call copies all previously concatenated data. Collect arrays in a list
; and concatenate once after the loop.
(call
  function: [
    (attribute
      attribute: (identifier) @dk_concatenate_attr
      (#eq? @dk_concatenate_attr "concatenate")
    )
    (identifier) @dk_concatenate_bare
    (#eq? @dk_concatenate_bare "concatenate")
  ]
) @dk_concatenate_call


; DK010 — .rechunk() to a pathological literal chunk.
;
; The same literal-only analysis as XR012, on the one dask call whose whole
; purpose is to choose a chunk shape. `.rechunk(1)` or `.rechunk((1, 1000))`
; pays a full shuffle to arrive at a graph with a task per element.
;
; Deliberately a separate pattern from DK008 rather than a branch inside it:
; DK008 is about *where* the rechunk happens and DK010 about *what* it asks
; for, and both can be true of one call.
(call
  function: (attribute
    attribute: (identifier) @dk_rechunk_spec_method
    (#eq? @dk_rechunk_spec_method "rechunk")
  )
  arguments: (argument_list) @dk_rechunk_spec_args
) @dk_rechunk_spec_call
