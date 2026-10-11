; Agda reads ```agda blocks and bare ``` blocks as code, and all of them as
; one module, so they form one combined Agda layer.
((raw_blck
  lang: (ident) @_lang
  (blob) @injection.content)
  (#eq? @_lang "agda")
  (#set! injection.language "agda")
  (#set! injection.combined))

((raw_blck
  !lang
  (blob) @injection.content)
  (#set! injection.language "agda")
  (#set! injection.combined))

; Other blocks in their own language, as the Typst extension does.
((raw_blck
  lang: (ident) @injection.language
  (blob) @injection.content)
  (#not-eq? @injection.language "agda"))

((comment) @injection.content
  (#set! injection.language "comment"))
