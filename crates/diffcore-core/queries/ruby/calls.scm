; Ruby call expression patterns

; Pattern 0: Simple function/method call — foo(), bar(x)
(call
  method: (identifier) @callee
  arguments: (argument_list) @args) @node

; Pattern 1: Method call on object — obj.method(), User.find(1)
(call
  receiver: (_)
  method: (identifier) @callee
  arguments: (argument_list) @args) @node

; Pattern 2: Method call on constant — User.find(1), MyClass.new
(call
  receiver: (constant) @_receiver
  method: (identifier) @callee
  arguments: (argument_list) @args) @node
