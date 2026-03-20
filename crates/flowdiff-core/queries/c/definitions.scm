; C definition patterns

; Function definition — int foo(int x) { ... }
(function_definition
  declarator: (function_declarator
    declarator: (identifier) @func_name)) @func_node

; Function definition with pointer return — int *foo() { ... }
(function_definition
  declarator: (pointer_declarator
    declarator: (function_declarator
      declarator: (identifier) @func_name))) @func_node

; Struct specifier — struct Foo { ... };
(struct_specifier
  name: (type_identifier) @struct_name) @struct_node

; Enum specifier — enum Color { RED, GREEN, BLUE };
(enum_specifier
  name: (type_identifier) @enum_name) @enum_node

; Union specifier — union Data { ... };
(union_specifier
  name: (type_identifier) @union_name) @union_node

; Type definition — typedef int MyInt;
(type_definition
  declarator: (type_identifier) @typedef_name) @typedef_node

; Global variable declaration — int global_var = 42;
(declaration
  declarator: (init_declarator
    declarator: (identifier) @global_name)) @global_node
