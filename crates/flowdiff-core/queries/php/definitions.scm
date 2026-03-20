; PHP definition patterns

; Method declaration — public function foo() {}
(method_declaration
  name: (name) @method_name) @method_node

; Function definition — function foo() {}
(function_definition
  name: (name) @func_name) @func_node

; Class declaration — class Foo {}
(class_declaration
  name: (name) @class_name) @class_node

; Interface declaration — interface IFoo {}
(interface_declaration
  name: (name) @iface_name) @iface_node

; Trait declaration — trait Foo {}
(trait_declaration
  name: (name) @trait_name) @trait_node

; Enum declaration — enum Foo {}
(enum_declaration
  name: (name) @enum_name) @enum_node

; Constant declaration — const FOO = 42;
(const_declaration
  (const_element
    (name) @const_name)) @const_node
