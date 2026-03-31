; Go definition patterns
; Each pattern uses distinct capture names so the engine can dispatch
; by capture-name presence instead of fragile pattern_index ordering.

; Function declaration — func Foo() {}
(function_declaration
  name: (identifier) @fn_name) @fn_node

; Method declaration — func (r *Receiver) Foo() {}
(method_declaration
  name: (field_identifier) @method_name) @method_node

; Type declaration with struct — type Foo struct {}
(type_declaration
  (type_spec
    name: (type_identifier) @struct_name
    type: (struct_type))) @struct_node

; Type declaration with interface — type Foo interface {}
(type_declaration
  (type_spec
    name: (type_identifier) @iface_name
    type: (interface_type))) @iface_node

; Type alias / other type declaration — type Foo = Bar, type Foo int
(type_declaration
  (type_spec
    name: (type_identifier) @type_name)) @type_node

; Const declaration — const Foo = 42
(const_declaration
  (const_spec
    name: (identifier) @const_name)) @const_node

; Var declaration — var foo int
(var_declaration
  (var_spec
    name: (identifier) @var_decl_name)) @var_decl_node
