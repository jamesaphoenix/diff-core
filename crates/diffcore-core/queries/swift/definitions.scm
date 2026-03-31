; Swift definition patterns
;
; class_declaration covers: struct, class, enum, extension, actor
; via the declaration_kind field

; Function declaration — func foo() { ... }
(function_declaration
  name: (simple_identifier) @func_name) @func_node

; Class/struct/enum/extension/actor declaration — struct Foo { ... }
(class_declaration
  name: (type_identifier) @class_name) @class_node

; Protocol declaration — protocol Foo { ... }
(protocol_declaration
  name: (type_identifier) @protocol_name) @protocol_node

; Protocol function declaration — func foo() inside protocol body
(protocol_function_declaration
  name: (simple_identifier) @proto_func_name) @proto_func_node

; Property declaration (top-level let/var) — let foo = 42
(property_declaration
  name: (pattern
    (simple_identifier) @prop_name)) @prop_node

; Type alias — typealias Foo = Bar
(typealias_declaration
  name: (type_identifier) @typealias_name) @typealias_node
