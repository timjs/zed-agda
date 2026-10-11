; From the Typst extension for Zed, https://github.com/WeetHet/typst.zed,
; commit 289ce59, under the Apache License 2.0.

(_
  "["
  "]" @end) @indent

(_
  "{"
  "}" @end) @indent

(_
  "("
  ")" @end) @indent

(_
  "$"
  "$" @end) @indent

((comment) @indent
  (#match? @indent "^/\\*"))
