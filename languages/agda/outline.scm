; The outline: each item shows its name, then its parameters and type,
; as in `List (A : Set) : Set` or `suc : ℕ → ℕ`. Zed joins the captured
; pieces with a space and nests items whose range lies inside another's.

; Type signatures, constructors included. Every name is an item of its own,
; with only the name as its range, so that the names of `tt ff : 𝔹` are
; siblings instead of one nested in the other.
(function
  (lhs
    (function_name
      (atom (qid) @name) @item))
  (rhs ":" @context (expr) @context))

; Record fields, one item per name, like constructors.
(signature
  (field_name) @name @item
  ":" @context
  (expr) @context)

(module
  (module_name) @name) @item

(postulate
  "postulate" @name) @item

; One pattern per kind, with optional parts: two patterns for the same
; declaration would give two items with the same range, one nested in the
; other.
(data_signature
  (data_name) @name
  [(typed_binding) (untyped_binding)]* @context
  ":" @context
  (expr) @context) @item

(data
  (data_name) @name
  [(typed_binding) (untyped_binding)]* @context
  ":"? @context
  (expr)? @context) @item

(record_signature
  (record_name) @name
  [(typed_binding) (untyped_binding)]* @context
  ":" @context
  (expr) @context) @item

(record
  (record_name) @name
  [(typed_binding) (untyped_binding)]* @context
  ":"? @context
  (expr)? @context) @item
