; Java definition patterns

; Method declaration — public void foo() {}
(method_declaration
  name: (identifier) @method_name) @method_node

; Constructor declaration — public Foo() {}
(constructor_declaration
  name: (identifier) @ctor_name) @ctor_node

; Class declaration — public class Foo {}
(class_declaration
  name: (identifier) @class_name) @class_node

; Interface declaration — public interface Foo {}
(interface_declaration
  name: (identifier) @iface_name) @iface_node

; Enum declaration — public enum Foo {}
(enum_declaration
  name: (identifier) @enum_name) @enum_node

; Annotation type declaration — public @interface Foo {}
(annotation_type_declaration
  name: (identifier) @annotation_name) @annotation_node

; Field declaration with variable declarator — private int foo;
(field_declaration
  declarator: (variable_declarator
    name: (identifier) @field_name)) @field_node

; Constant (static final field) — public static final int FOO = 42;
; Captured via field_declaration above, distinguished in Rust code by modifiers
