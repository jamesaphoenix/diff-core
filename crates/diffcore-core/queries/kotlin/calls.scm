; Kotlin call expression patterns
;
; AST structure:
;   (call_expression
;     (identifier) @callee          — simple call: foo()
;     (value_arguments) @args
;   )
;   (call_expression
;     (navigation_expression
;       (_)
;       (identifier) @callee)       — method call: obj.method()
;     (value_arguments) @args
;   )

; Pattern 0: Simple function call — foo(x, y)
(call_expression
  (identifier) @callee
  (value_arguments) @args) @node

; Pattern 1: Method call on object — obj.method(x)
(call_expression
  (navigation_expression
    (_)
    (identifier) @callee)
  (value_arguments) @args) @node
