; Swift variable assignment from call patterns
; Used for data flow tracing: let x = foo()
;
; AST structure:
;   (property_declaration
;     name: (pattern (simple_identifier))
;     value: (call_expression ...))

; Pattern 0: let/var assignment from simple call — let result = greet(name: "World")
(property_declaration
  name: (pattern
    (simple_identifier) @var_name)
  value: (call_expression
    (simple_identifier) @callee)) @node

; Pattern 1: let/var assignment from method call — let users = controller.findAll()
(property_declaration
  name: (pattern
    (simple_identifier) @var_name)
  value: (call_expression
    (navigation_expression
      (navigation_suffix
        (simple_identifier) @callee)))) @node
