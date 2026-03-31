; PHP call expression patterns

; Pattern 0: Function call — foo(), greet('World')
(function_call_expression
  function: (name) @callee
  arguments: (arguments) @args) @node

; Pattern 1: Method call — $obj->method()
(member_call_expression
  name: (name) @callee
  arguments: (arguments) @args) @node

; Pattern 2: Static method call — User::find(), ClassName::method()
(scoped_call_expression
  name: (name) @callee
  arguments: (arguments) @args) @node

; Pattern 3: Object creation — new Foo()
(object_creation_expression
  (name) @callee
  (arguments) @args) @node
