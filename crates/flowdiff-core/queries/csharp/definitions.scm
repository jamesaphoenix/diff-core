; C# definition patterns

; Method declaration — public void Foo() {}
(method_declaration
  name: (identifier) @method_name) @method_node

; Constructor declaration — public MyClass() {}
(constructor_declaration
  name: (identifier) @ctor_name) @ctor_node

; Class declaration — public class Foo {}
(class_declaration
  name: (identifier) @class_name) @class_node

; Struct declaration — public struct Foo {}
(struct_declaration
  name: (identifier) @struct_name) @struct_node

; Interface declaration — public interface IFoo {}
(interface_declaration
  name: (identifier) @iface_name) @iface_node

; Enum declaration — public enum Foo {}
(enum_declaration
  name: (identifier) @enum_name) @enum_node

; Record declaration — public record Foo {}
(record_declaration
  name: (identifier) @record_name) @record_node

; Property declaration — public int Foo { get; set; }
(property_declaration
  name: (identifier) @prop_name) @prop_node

; Field declaration — private int _foo;
(field_declaration
  (variable_declaration
    (variable_declarator
      name: (identifier) @field_name))) @field_node

; Delegate declaration — public delegate void MyHandler(object sender, EventArgs e);
(delegate_declaration
  name: (identifier) @delegate_name) @delegate_node
