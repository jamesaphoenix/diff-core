; Java call expression patterns

; Pattern 0: Method invocation — foo(), obj.bar(), Foo.baz()
(method_invocation
  name: (identifier) @callee
  arguments: (argument_list) @args) @node

; Pattern 1: Object creation — new Foo()
(object_creation_expression
  type: (_) @callee
  arguments: (argument_list) @args) @node
