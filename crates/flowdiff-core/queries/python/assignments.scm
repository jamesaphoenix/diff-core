; Python assignment from call patterns
; Used for data flow tracing: x = func_a() or x = await func_a()

; Pattern 0: Variable assigned from direct call — x = foo()
(assignment
  left: (identifier) @var_name
  right: (call
    function: (_) @callee)) @node

; Pattern 1: Variable assigned from awaited call — x = await foo()
(assignment
  left: (identifier) @var_name
  right: (await
    (call
      function: (_) @callee))) @node
