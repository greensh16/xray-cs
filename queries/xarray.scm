; XR001 — open_dataset / open_mfdataset called without chunks= keyword argument.
; We capture the entire call so Rust can inspect its argument_list for a
; "chunks" keyword_argument child.
(call
  function: [
    (identifier) @fn_bare
    (attribute attribute: (identifier) @fn_attr)
  ]
  arguments: (argument_list) @args
  (#match? @fn_bare "^open_(mf)?dataset$")
  (#match? @fn_attr "^open_(mf)?dataset$")
) @xr_open_call


; XR002 — .values accessed on any expression.
; Heuristic: if xarray is imported, .values on an array-like is almost always
; wrong — it materialises the entire array and drops coordinate metadata.
(attribute
  object: (_) @xr_values_obj
  attribute: (identifier) @xr_values_attr
  (#eq? @xr_values_attr "values")
) @xr_values_access


; XR003 — for loop iterating directly over a Dataset or DataArray dimension.
; Pattern: `for <var> in <expr>.<dim>:` where <dim> is a known coord name.
; We capture the iterable so the rule can check whether it's an attribute
; access (ds.time, ds.lat, da.x, etc.).
(for_statement
  left: (_) @xr_loop_var
  right: (attribute
    object: (_) @xr_loop_obj
    attribute: (identifier) @xr_loop_dim
  ) @xr_loop_iter
) @xr_for_dim


; XR004 — .sel() called with a plain float literal as a positional or keyword value.
; Float coords in xarray use inexact comparison unless method= is supplied.
(call
  function: (attribute
    attribute: (identifier) @xr_sel_method
    (#eq? @xr_sel_method "sel")
  )
  arguments: (argument_list
    [
      (float) @xr_sel_float_pos
      ; Negative literals parse as unary_operator, not float — without this
      ; southern latitudes and western longitudes were silently skipped.
      (unary_operator argument: (float)) @xr_sel_float_pos
      (keyword_argument
        value: (float) @xr_sel_float_kw
      )
      (keyword_argument
        value: (unary_operator argument: (float)) @xr_sel_float_kw
      )
    ]
  )
) @xr_sel_call


; XR005 — .compute() call.  Rust uses is_inside_for_loop() to restrict
; this to loop bodies — triggers the full dask graph on every iteration.
(call
  function: (attribute
    attribute: (identifier) @xr_compute_in_loop
    (#eq? @xr_compute_in_loop "compute")
  )
) @xr_compute_call


; XR006 — ds.to_array() / ds.to_dataarray() called without dim=.
; Creates an unnamed concat dimension called "variable", which makes
; downstream .sel() and indexing code fragile and error-prone.
(call
  function: (attribute
    attribute: (identifier) @xr_to_array_attr
    (#match? @xr_to_array_attr "^(to_array|to_dataarray)$")
  )
) @xr_to_array_call


; XR007 — xr.concat call.  Rust uses is_inside_for_loop() to restrict
; this to loop bodies — same O(n²) problem as np.concatenate in a loop.
(call
  function: [
    (attribute
      attribute: (identifier) @xr_concat_method
      (#eq? @xr_concat_method "concat")
    )
    (identifier) @xr_concat_bare
    (#eq? @xr_concat_bare "concat")
  ]
) @xr_concat_call


; XR008 — open_mfdataset without parallel=True.
; open_mfdataset opens many files serially by default; parallel=True uses
; dask.delayed to open files concurrently, which can be orders of magnitude
; faster on large ensembles. We capture the call; Rust checks for parallel=.
(call
  function: [
    (attribute
      attribute: (identifier) @xr_mfdataset_attr
      (#eq? @xr_mfdataset_attr "open_mfdataset")
    )
    (identifier) @xr_mfdataset_bare
    (#eq? @xr_mfdataset_bare "open_mfdataset")
  ]
) @xr_mfdataset_call


; XR009 — apply_ufunc with dask="allowed".
; dask="allowed" silently falls back to serial execution when dask is not
; installed or when arrays are not dask-backed. Use dask="parallelized" for
; correct distributed operation. We capture the call; Rust inspects the
; dask= kwarg value.
(call
  function: [
    (attribute
      attribute: (identifier) @xr_apply_ufunc_attr
      (#eq? @xr_apply_ufunc_attr "apply_ufunc")
    )
    (identifier) @xr_apply_ufunc_bare
    (#eq? @xr_apply_ufunc_bare "apply_ufunc")
  ]
) @xr_apply_ufunc_call


; XR010 — xr.merge() inside a loop.
; Calling merge() on each iteration builds an ever-growing Dataset; the full
; merge cost (alignment, coord broadcasting) is paid O(n) times. Collect the
; list first, then call merge() once outside the loop.
(call
  function: [
    (attribute
      attribute: (identifier) @xr_merge_attr
      (#eq? @xr_merge_attr "merge")
    )
    (identifier) @xr_merge_bare
    (#eq? @xr_merge_bare "merge")
  ]
) @xr_merge_call


; XR011 — ds.to_netcdf() without encoding= keyword argument.
; Without encoding= the variable dtypes and fill values are written as-is,
; typically float64 with no compression. Specifying encoding= with dtype,
; scale_factor, add_offset, and zlib can reduce file sizes by 5-10x.
(call
  function: (attribute
    attribute: (identifier) @xr_to_netcdf_attr
    (#eq? @xr_to_netcdf_attr "to_netcdf")
  )
) @xr_to_netcdf_call


; XR012 — a literal chunk length of 1 (or a trivially small literal chunk) in
; `chunks=` or `.chunk()`.
;
; Shapes need a runtime; chunk *arguments* do not. `chunks={"time": 1}` on a
; 40-year hourly dataset is 350k tasks, each one a separate Lustre metadata
; round-trip — the scheduler and the filesystem do all the work and the CPUs
; sit idle. One pattern covers both spellings; Rust picks the chunk spec out
; of the argument list and classifies the literal.
(call
  function: [
    (identifier) @xr_chunkspec_bare
    (attribute attribute: (identifier) @xr_chunkspec_attr)
  ]
  arguments: (argument_list) @xr_chunkspec_args
  (#match? @xr_chunkspec_bare "^(open_dataset|open_mfdataset|open_zarr|chunk)$")
  (#match? @xr_chunkspec_attr "^(open_dataset|open_mfdataset|open_zarr|chunk)$")
) @xr_chunkspec_call
