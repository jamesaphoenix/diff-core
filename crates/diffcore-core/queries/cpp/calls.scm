; C++ call expression patterns
;
; AST structure:
;   (call_expression
;     function: (identifier)                   — foo(x)
;     arguments: (argument_list))
;   (call_expression
;     function: (field_expression              — obj.method(x) or obj->method(x)
;       field: (field_identifier))
;     arguments: (argument_list))
;   (call_expression
;     function: (qualified_identifier          — ns::func(x)
;       name: (identifier))
;     arguments: (argument_list))

; Pattern 0: Simple function call — foo(x, y)
(call_expression
  function: (identifier) @callee
  arguments: (argument_list) @args) @node

; Pattern 1: Member call — obj.method(x) or ptr->method(x)
(call_expression
  function: (field_expression
    field: (field_identifier) @callee)
  arguments: (argument_list) @args) @node

; Pattern 2: Qualified call — std::sort(a, b) or ns::func()
(call_expression
  function: (qualified_identifier
    name: (identifier) @callee)
  arguments: (argument_list) @args) @node

; Pattern 3: Template function call — make_shared<T>(args)
(call_expression
  function: (template_function
    name: (identifier) @callee)
  arguments: (argument_list) @args) @node
