; Go variable assignment from call patterns
; Used for data flow tracing: x := foo() or var x = foo()

; Pattern 0: Short variable declaration from call — x := foo()
(short_var_declaration
  left: (expression_list
    (identifier) @var_name)
  right: (expression_list
    (call_expression
      function: (_) @callee))) @node

; Pattern 1: Var declaration with call value — var x = foo()
(var_declaration
  (var_spec
    name: (identifier) @var_name
    value: (expression_list
      (call_expression
        function: (_) @callee)))) @node
