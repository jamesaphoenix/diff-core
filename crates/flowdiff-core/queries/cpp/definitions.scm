; C++ definition patterns

; Function definition — int foo(int x) { ... }
(function_definition
  declarator: (function_declarator
    declarator: (identifier) @func_name)) @func_node

; Function definition with pointer return — int* foo() { ... }
(function_definition
  declarator: (pointer_declarator
    declarator: (function_declarator
      declarator: (identifier) @func_name))) @func_node

; Function definition with qualified name — void MyClass::method() { ... }
(function_definition
  declarator: (function_declarator
    declarator: (qualified_identifier
      name: (identifier) @method_name))) @method_node

; Class specifier — class Foo { ... };
(class_specifier
  name: (type_identifier) @class_name) @class_node

; Struct specifier — struct Bar { ... };
(struct_specifier
  name: (type_identifier) @struct_name) @struct_node

; Enum specifier — enum Color { RED, GREEN };
(enum_specifier
  name: (type_identifier) @enum_name) @enum_node

; Namespace definition — namespace foo { ... }
(namespace_definition
  name: (namespace_identifier) @namespace_name) @namespace_node

; Type alias — using MyType = std::vector<int>;
(alias_declaration
  name: (type_identifier) @alias_name) @alias_node

; Template declaration wrapping a function
(template_declaration
  (function_definition
    declarator: (function_declarator
      declarator: (identifier) @template_func_name))) @template_func_node

; Template declaration wrapping a class
(template_declaration
  (class_specifier
    name: (type_identifier) @template_class_name)) @template_class_node
