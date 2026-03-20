; Go call expression patterns

; Pattern 0: Any call expression — foo(), pkg.Bar(), obj.Method()
(call_expression
  function: (_) @callee
  arguments: (argument_list) @args) @node
