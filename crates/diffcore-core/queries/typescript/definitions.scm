; TypeScript/JavaScript definition patterns
; Each pattern uses distinct capture names so the engine can dispatch
; by capture-name presence instead of fragile pattern_index ordering.

; Function declaration
(function_declaration
  name: (identifier) @fn_name) @fn_node

; Generator function declaration
(generator_function_declaration
  name: (identifier) @gen_name) @gen_node

; Class declaration
(class_declaration
  name: (_) @class_name) @class_node

; Abstract class declaration
(abstract_class_declaration
  name: (_) @abstract_name) @abstract_node

; Interface declaration
(interface_declaration
  name: (_) @iface_name) @iface_node

; Type alias declaration
(type_alias_declaration
  name: (_) @type_name) @type_node

; Variable declarator with arrow function value
(variable_declarator
  name: (identifier) @arrow_name
  value: (arrow_function)) @arrow_node

; Variable declarator with function expression value
(variable_declarator
  name: (identifier) @fn_expr_name
  value: (function_expression)) @fn_expr_node

; Variable declarator with non-function value (constant)
(variable_declarator
  name: (identifier) @const_name
  value: (_) @const_value) @const_node

; Class method definition
(method_definition
  name: (_) @method_name) @method_node
