; SP001 — scipy.integrate.quad() inside a loop.
;
; quad() is a scalar quadrature routine: it integrates one function over one
; interval and returns one number. Calling it per element re-enters the Fortran
; QUADPACK driver, re-allocates its workspace and re-runs its error control on
; every iteration. quad_vec() does the whole vector in one adaptive pass,
; sharing the subdivision across components.
;
; Loop context is decided in Rust with `is_inside_loop`, as with every other
; loop-sensitive rule.
(call
  function: [
    (attribute
      attribute: (identifier) @sp_quad_attr
      (#eq? @sp_quad_attr "quad")
    )
    (identifier) @sp_quad_bare
    (#eq? @sp_quad_bare "quad")
  ]
) @sp_quad_call


; SP002 — scipy.linalg.inv() / scipy.linalg.pinv2-style explicit inversion.
;
; Forming an inverse to then multiply by it is both slower and less accurate
; than solving the system directly: inv() is an LU factorisation plus n
; triangular solves, where solve() is the same factorisation and one solve,
; and the explicit inverse squares the condition number's effect on the
; result.
(call
  function: [
    (attribute
      attribute: (identifier) @sp_inv_attr
      (#eq? @sp_inv_attr "inv")
    )
    (identifier) @sp_inv_bare
    (#eq? @sp_inv_bare "inv")
  ]
) @sp_inv_call
