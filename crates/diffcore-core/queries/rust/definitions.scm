; Rust definition patterns
; Each pattern uses distinct capture names so the engine can dispatch
; by capture-name presence.

; Function declaration — fn foo() {}
(function_item
  name: (identifier) @fn_name) @fn_node

; Struct definition — struct Foo {}
(struct_item
  name: (type_identifier) @struct_name) @struct_node

; Enum definition — enum Foo {}
(enum_item
  name: (type_identifier) @enum_name) @enum_node

; Trait definition — trait Foo {}
(trait_item
  name: (type_identifier) @trait_name) @trait_node

; Type alias — type Foo = Bar;
(type_item
  name: (type_identifier) @type_name) @type_node

; Const declaration — const FOO: i32 = 42;
(const_item
  name: (identifier) @const_name) @const_node

; Static declaration — static FOO: i32 = 42;
(static_item
  name: (identifier) @static_name) @static_node

; Impl block method — impl Foo { fn bar() {} }
(impl_item
  body: (declaration_list
    (function_item
      name: (identifier) @method_name) @method_node))

; Macro definition — macro_rules! foo { }
(macro_definition
  name: (identifier) @macro_name) @macro_node
