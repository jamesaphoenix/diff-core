; PHP variable assignment from call patterns
; Used for data flow tracing: $x = foo();

; Pattern 0: Assignment from function call — $result = greet('World');
(expression_statement
  (assignment_expression
    left: (variable_name
      (name) @var_name)
    right: (function_call_expression
      function: (name) @callee))) @node

; Pattern 1: Assignment from method call — $val = $obj->findAll();
(expression_statement
  (assignment_expression
    left: (variable_name
      (name) @var_name)
    right: (member_call_expression
      name: (name) @callee))) @node

; Pattern 2: Assignment from static call — $user = User::find(1);
(expression_statement
  (assignment_expression
    left: (variable_name
      (name) @var_name)
    right: (scoped_call_expression
      name: (name) @callee))) @node

; Pattern 3: Assignment from object creation — $obj = new UserService();
(expression_statement
  (assignment_expression
    left: (variable_name
      (name) @var_name)
    right: (object_creation_expression
      (name) @callee))) @node
