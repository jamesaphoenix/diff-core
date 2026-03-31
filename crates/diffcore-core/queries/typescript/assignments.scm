; TypeScript/JavaScript variable assignment from call patterns
; Used for data flow tracing: const x = funcA() or const x = await funcA()

; Pattern 0: Variable assigned from direct call — const x = foo()
(variable_declarator
  name: (identifier) @var_name
  value: (call_expression
    function: (_) @callee)) @node

; Pattern 1: Variable assigned from awaited call — const x = await foo()
(variable_declarator
  name: (identifier) @var_name
  value: (await_expression
    (call_expression
      function: (_) @callee))) @node
