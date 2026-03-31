; C++ variable assignment from call patterns
; Used for data flow tracing: auto x = foo();
;
; AST structure:
;   (declaration
;     declarator: (init_declarator
;       declarator: (identifier)
;       value: (call_expression ...)))

; Pattern 0: Variable init from simple call — auto result = process();
(declaration
  declarator: (init_declarator
    declarator: (identifier) @var_name
    value: (call_expression
      function: (identifier) @callee))) @node

; Pattern 1: Variable init from member call — auto result = obj.get();
(declaration
  declarator: (init_declarator
    declarator: (identifier) @var_name
    value: (call_expression
      function: (field_expression
        field: (field_identifier) @callee)))) @node

; Pattern 2: Variable init from qualified call — auto v = std::make_unique<T>();
(declaration
  declarator: (init_declarator
    declarator: (identifier) @var_name
    value: (call_expression
      function: (qualified_identifier
        name: (identifier) @callee)))) @node
