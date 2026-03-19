; TypeScript/JavaScript definition patterns
; Pattern index determines the SymbolKind.

; Pattern 0: Function declaration
(function_declaration
  name: (identifier) @name) @node

; Pattern 1: Generator function declaration
(generator_function_declaration
  name: (identifier) @name) @node

; Pattern 2: Class declaration
(class_declaration
  name: (_) @name) @node

; Pattern 3: Abstract class declaration
(abstract_class_declaration
  name: (_) @name) @node

; Pattern 4: Interface declaration
(interface_declaration
  name: (_) @name) @node

; Pattern 5: Type alias declaration
(type_alias_declaration
  name: (_) @name) @node

; Pattern 6: Variable declarator with function/arrow value
(variable_declarator
  name: (identifier) @name
  value: (arrow_function)) @node

; Pattern 7: Variable declarator with function expression value
(variable_declarator
  name: (identifier) @name
  value: (function_expression)) @node

; Pattern 8: Variable declarator with non-function value (constant)
(variable_declarator
  name: (identifier) @name
  value: (_) @value) @node

; Pattern 9: Class method definition
(method_definition
  name: (_) @name) @node
