; C call expression patterns
;
; AST structure:
;   (call_expression
;     function: (identifier)               — foo(x)
;     arguments: (argument_list))
;   (call_expression
;     function: (field_expression          — obj->method(x) or obj.method(x)
;       field: (field_identifier))
;     arguments: (argument_list))

; Pattern 0: Simple function call — foo(x, y)
(call_expression
  function: (identifier) @callee
  arguments: (argument_list) @args) @node

; Pattern 1: Field/pointer member call — obj->func(x) or obj.func(x)
(call_expression
  function: (field_expression
    field: (field_identifier) @callee)
  arguments: (argument_list) @args) @node
