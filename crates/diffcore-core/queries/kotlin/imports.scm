; Kotlin import patterns
; Kotlin uses `import pkg.Class`, `import pkg.Class as Alias`, `import pkg.*`
;
; AST structure:
;   (import
;     (qualified_identifier (identifier)+ )
;     [(as (identifier))]  — alias
;     [(. *)]              — wildcard
;   )

; Pattern 0: Regular import — import com.example.Foo
(import
  (qualified_identifier) @source) @stmt

; Pattern 1: Aliased import — import com.example.Foo as Bar
(import
  (qualified_identifier) @alias_source
  (identifier) @alias_name) @stmt

; Pattern 2: Wildcard import — import com.example.*
(import
  (qualified_identifier) @wildcard_source
  "*" @wildcard) @stmt
