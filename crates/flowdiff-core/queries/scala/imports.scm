; Scala import patterns
; Scala uses `import pkg.Class`, `import pkg.{A, B}`, `import pkg._`
;
; Note: Scala's tree-sitter grammar represents the import path as
; a flat list of (identifier) children separated by `.` tokens.
; Named imports use (namespace_selectors), wildcard uses (namespace_wildcard).
; The Rust extractor reconstructs the dotted path from the AST structure.

; Pattern 0: All import declarations
(import_declaration) @stmt
