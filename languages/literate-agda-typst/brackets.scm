; From the Typst extension for Zed, https://github.com/WeetHet/typst.zed,
; commit 289ce59, under the Apache License 2.0.

("(" @open
  ")" @close)

("[" @open
  "]" @close)

("{" @open
  "}" @close)

("\"" @open
  "\"" @close
  (#set! rainbow.exclude))

("`" @open
  "`" @close
  (#set! rainbow.exclude))

("```" @open
  "```" @close
  (#set! rainbow.exclude))

("$" @open
  "$" @close
  (#set! rainbow.exclude))
