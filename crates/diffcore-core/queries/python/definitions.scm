; Python definition patterns
; Each pattern uses distinct capture names so the engine can dispatch
; by capture-name presence instead of fragile pattern_index ordering.

; Function definition
(function_definition
  name: (identifier) @fn_name) @fn_node

; Class definition
(class_definition
  name: (identifier) @class_name) @class_node

; Decorated function
(decorated_definition
  definition: (function_definition
    name: (identifier) @decorated_fn_name)) @decorated_fn_node

; Decorated class
(decorated_definition
  definition: (class_definition
    name: (identifier) @decorated_class_name)) @decorated_class_node

; Class method (function inside class body)
(class_definition
  body: (block
    (function_definition
      name: (identifier) @method_name) @method_node))

; Decorated class method
(class_definition
  body: (block
    (decorated_definition
      definition: (function_definition
        name: (identifier) @decorated_method_name) @decorated_method_node)))
