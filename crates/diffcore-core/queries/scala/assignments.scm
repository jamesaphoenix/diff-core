; Scala variable assignment from call patterns
; Used for data flow tracing: val x = foo()
;
; AST structure:
;   (val_definition
;     (identifier) @var_name
;     (call_expression ...))
;   (var_definition
;     (identifier) @var_name
;     (call_expression ...))

; Pattern 0: val assignment from simple call — val result = getUser("123")
(val_definition
  (identifier) @var_name
  (call_expression
    (identifier) @callee)) @node

; Pattern 1: val assignment from method call — val user = repo.findById(id)
(val_definition
  (identifier) @var_name
  (call_expression
    (field_expression
      (_)
      (identifier) @callee))) @node

; Pattern 2: var assignment from simple call — var count = fetchCount()
(var_definition
  (identifier) @var_name
  (call_expression
    (identifier) @callee)) @node

; Pattern 3: var assignment from method call — var data = service.process(input)
(var_definition
  (identifier) @var_name
  (call_expression
    (field_expression
      (_)
      (identifier) @callee))) @node
