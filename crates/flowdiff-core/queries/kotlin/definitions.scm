; Kotlin definition patterns
;
; AST structure uses `identifier` (not `simple_identifier` or `type_identifier`)

; Function declaration — fun foo() { ... }
(function_declaration
  (identifier) @func_name) @func_node

; Class declaration — class Foo { ... } / data class Foo(...) / sealed class Foo
(class_declaration
  (identifier) @class_name) @class_node

; Object declaration — object Foo { ... }
(object_declaration
  (identifier) @object_name) @object_node

; Property declaration (top-level val/var) — val FOO = 42
(property_declaration
  (variable_declaration
    (identifier) @prop_name)) @prop_node

; Type alias — typealias Foo = Bar
(type_alias
  (identifier) @typealias_name) @typealias_node
