; Swift call expression patterns
;
; AST structure:
;   (call_expression
;     (simple_identifier)         — simple call: foo()
;     (call_suffix
;       (value_arguments)))
;   (call_expression
;     (navigation_expression      — method call: obj.method()
;       (simple_identifier) [target]
;       (navigation_suffix
;         (simple_identifier) [suffix]))
;     (call_suffix
;       (value_arguments)))

; Pattern 0: Simple function call — foo(x, y)
(call_expression
  (simple_identifier) @callee
  (call_suffix
    (value_arguments) @args)) @node

; Pattern 1: Method call — obj.method(x)
(call_expression
  (navigation_expression
    (navigation_suffix
      (simple_identifier) @callee))
  (call_suffix
    (value_arguments) @args)) @node
