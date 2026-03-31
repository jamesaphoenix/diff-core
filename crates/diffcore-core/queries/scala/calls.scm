; Scala call expression patterns
;
; AST structure:
;   (call_expression
;     (identifier) @callee          — simple call: foo()
;     (arguments) @args
;   )
;   (call_expression
;     (field_expression
;       (_)
;       (identifier) @callee)       — method call: obj.method()
;     (arguments) @args
;   )

; Pattern 0: Simple function call — foo(x, y)
(call_expression
  (identifier) @callee
  (arguments) @args) @node

; Pattern 1: Method call on object — obj.method(x)
(call_expression
  (field_expression
    (_)
    (identifier) @callee)
  (arguments) @args) @node
