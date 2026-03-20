; C# variable assignment from call patterns
; Used for data flow tracing: var x = Foo();

; Pattern 0: Local variable with direct invocation — var x = Foo();
(local_declaration_statement
  (variable_declaration
    (variable_declarator
      name: (identifier) @var_name
      (invocation_expression
        function: (identifier) @callee)))) @node

; Pattern 1: Local variable with member invocation — var x = obj.Method();
(local_declaration_statement
  (variable_declaration
    (variable_declarator
      name: (identifier) @var_name
      (invocation_expression
        function: (member_access_expression
          name: (identifier) @callee))))) @node

; Pattern 2: Local variable with object creation — var x = new Foo();
(local_declaration_statement
  (variable_declaration
    (variable_declarator
      name: (identifier) @var_name
      (object_creation_expression
        type: (_) @callee)))) @node
