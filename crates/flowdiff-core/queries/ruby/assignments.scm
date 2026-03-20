; Ruby variable assignment from call patterns
; Used for data flow tracing: x = foo()

; Pattern 0: Assignment from simple call — result = greet('World')
(assignment
  left: (identifier) @var_name
  right: (call
    method: (identifier) @callee)) @node

; Pattern 1: Assignment from method call on object — val = obj.find_all()
(assignment
  left: (identifier) @var_name
  right: (call
    receiver: (_)
    method: (identifier) @callee)) @node

; Pattern 2: Instance variable assignment from call — @user = User.find(1)
(assignment
  left: (instance_variable) @var_name
  right: (call
    method: (identifier) @callee)) @node

; Pattern 3: Instance variable assignment from method call — @user = repo.find(id)
(assignment
  left: (instance_variable) @var_name
  right: (call
    receiver: (_)
    method: (identifier) @callee)) @node
