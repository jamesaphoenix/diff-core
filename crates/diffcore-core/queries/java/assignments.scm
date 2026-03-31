; Java variable assignment from call patterns
; Used for data flow tracing: Type x = foo();

; Pattern 0: Local variable declaration with method call — Type x = foo();
(local_variable_declaration
  declarator: (variable_declarator
    name: (identifier) @var_name
    value: (method_invocation
      name: (identifier) @callee))) @node

; Pattern 1: Local variable declaration with object creation — Foo x = new Foo();
(local_variable_declaration
  declarator: (variable_declarator
    name: (identifier) @var_name
    value: (object_creation_expression
      type: (_) @callee))) @node
