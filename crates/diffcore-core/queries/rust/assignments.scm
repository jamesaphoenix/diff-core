; Rust variable assignment from call patterns
; Used for data flow tracing: let x = foo();

; Pattern 0: let binding from call — let x = foo();
(let_declaration
  pattern: (identifier) @var_name
  value: (call_expression
    function: (_) @callee)) @node

; Pattern 1: let binding with type from call — let x: Type = foo();
(let_declaration
  pattern: (identifier) @var_name
  value: (call_expression
    function: (_) @callee)) @node
