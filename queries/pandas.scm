; PD001 — DataFrame.iterrows() reached from inside an enclosing loop.
;
; NP001 flags iterrows() wherever it appears. This is the worse case: the call
; sits in the body of another loop, so the row-by-row Python iteration is paid
; once per outer iteration. Loop context is decided in Rust with
; `is_inside_loop`, which is what makes `for i, row in df.iterrows():` at top
; level (the call is in the loop *header*, evaluated once) come out clean while
; the nested spelling does not.
(call
  function: (attribute
    attribute: (identifier) @pd_iterrows
    (#eq? @pd_iterrows "iterrows")
  )
) @pd_iterrows_call


; PD002 — DataFrame.append() — removed in pandas 2.0, not merely deprecated.
;
; `list.append` is the single most common method call in Python, so this rule
; fires only when the receiver is a *known* pandas object. That inverts the
; usual "unknown receiver keeps the old behaviour" convention on purpose: for
; a brand-new rule the previous behaviour is silence, and an unknown `.append`
; is overwhelmingly a list.
(call
  function: (attribute
    object: (_) @pd_append_recv
    attribute: (identifier) @pd_append_attr
    (#eq? @pd_append_attr "append")
  )
) @pd_append_call


; PD003 — chained assignment: `df[...][...] = ...`.
;
; The first subscript may return a copy, in which case the assignment lands on
; a temporary and is thrown away — pandas raises SettingWithCopyWarning and, in
; pandas 3.0's copy-on-write, silently does nothing at all. NP005 flags the
; *read* form; this is the form that loses data.
(assignment
  left: (subscript
    value: (subscript) @pd_chained_inner
  ) @pd_chained_target
) @pd_chained_assign


; PD004 — pd.read_csv() without dtype=.
;
; Without dtype= pandas infers each column's type by scanning the whole file,
; then frequently lands on object or float64 where int32 would do. On a
; multi-GB CSV the inference pass alone can dominate the read.
(call
  function: [
    (attribute
      attribute: (identifier) @pd_read_csv_attr
      (#eq? @pd_read_csv_attr "read_csv")
    )
    (identifier) @pd_read_csv_bare
    (#eq? @pd_read_csv_bare "read_csv")
  ]
  arguments: (argument_list) @pd_read_csv_args
) @pd_read_csv_call


; PD005 — .to_csv() without index=False.
;
; The default writes the DataFrame index as a nameless leading column, which
; reappears as `Unnamed: 0` the next time anything reads the file.
(call
  function: (attribute
    attribute: (identifier) @pd_to_csv_attr
    (#eq? @pd_to_csv_attr "to_csv")
  )
  arguments: (argument_list) @pd_to_csv_args
) @pd_to_csv_call
