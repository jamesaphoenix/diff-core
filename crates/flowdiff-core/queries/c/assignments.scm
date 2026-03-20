; C variable assignment from call patterns
; Used for data flow tracing: int x = foo();
;
; AST structure:
;   (declaration
;     declarator: (init_declarator
;       declarator: (identifier)
;       value: (call_expression
;         function: (identifier)
;         arguments: (argument_list))))

; Pattern 0: Variable init from simple call — int result = process();
(declaration
  declarator: (init_declarator
    declarator: (identifier) @var_name
    value: (call_expression
      function: (identifier) @callee))) @node

; Pattern 1: Variable init from field call — int result = obj->get();
(declaration
  declarator: (init_declarator
    declarator: (identifier) @var_name
    value: (call_expression
      function: (field_expression
        field: (field_identifier) @callee)))) @node
