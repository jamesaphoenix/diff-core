; Kotlin variable assignment from call patterns
; Used for data flow tracing: val x = foo()
;
; AST structure:
;   (property_declaration
;     (variable_declaration (identifier) @var_name)
;     (call_expression ...))

; Pattern 0: val/var assignment from simple call — val result = greet("World")
(property_declaration
  (variable_declaration
    (identifier) @var_name)
  (call_expression
    (identifier) @callee)) @node

; Pattern 1: val/var assignment from method call — val user = repo.findById(id)
(property_declaration
  (variable_declaration
    (identifier) @var_name)
  (call_expression
    (navigation_expression
      (_)
      (identifier) @callee))) @node
