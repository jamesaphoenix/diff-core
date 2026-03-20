; Scala definition patterns
;
; AST node types:
;   function_definition  — def foo() = ...
;   function_declaration — def foo(): Type (abstract, no body)
;   class_definition     — class Foo { ... } / case class Foo(...)
;   trait_definition     — trait Foo { ... }
;   object_definition    — object Foo { ... }
;   val_definition       — val x = ...
;   var_definition       — var x = ...
;   type_definition      — type Foo = Bar

; Function definition — def foo() = expr
(function_definition
  (identifier) @func_name) @func_node

; Function declaration (abstract) — def foo(): Type
(function_declaration
  (identifier) @func_name) @func_node

; Class definition — class Foo { ... } / case class Foo(...)
(class_definition
  (identifier) @class_name) @class_node

; Trait definition — trait Foo { ... } / sealed trait Foo
(trait_definition
  (identifier) @trait_name) @trait_node

; Object definition — object Foo { ... }
(object_definition
  (identifier) @object_name) @object_node

; Val definition (top-level or member) — val x = 42
(val_definition
  (identifier) @prop_name) @prop_node

; Var definition (top-level or member) — var x = 42
(var_definition
  (identifier) @prop_name) @prop_node

; Type alias — type Foo = Bar
(type_definition
  (type_identifier) @typealias_name) @typealias_node
