; C# call expression patterns

; Pattern 0: Direct method invocation — Foo(), Bar.Baz()
(invocation_expression
  function: (identifier) @callee
  arguments: (argument_list) @args) @node

; Pattern 1: Member method invocation — obj.Method()
(invocation_expression
  function: (member_access_expression
    name: (identifier) @callee)
  arguments: (argument_list) @args) @node

; Pattern 2: Object creation — new Foo()
(object_creation_expression
  type: (_) @callee
  arguments: (argument_list) @args) @node
