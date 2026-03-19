; Python definition patterns
; Pattern index determines the SymbolKind.

; Pattern 0: Function definition
(function_definition
  name: (identifier) @name) @node

; Pattern 1: Class definition
(class_definition
  name: (identifier) @name) @node

; Pattern 2: Decorated function
(decorated_definition
  definition: (function_definition
    name: (identifier) @name)) @node

; Pattern 3: Decorated class
(decorated_definition
  definition: (class_definition
    name: (identifier) @name)) @node

; Pattern 4: Class method (function inside class body)
(class_definition
  body: (block
    (function_definition
      name: (identifier) @method_name) @method_node))

; Pattern 5: Decorated class method
(class_definition
  body: (block
    (decorated_definition
      definition: (function_definition
        name: (identifier) @decorated_method_name) @decorated_method_node)))
