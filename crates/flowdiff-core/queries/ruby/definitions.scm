; Ruby definition patterns

; Method definition — def foo() ... end
(method
  name: (identifier) @method_name) @method_node

; Singleton method — def self.foo() ... end
(singleton_method
  name: (identifier) @singleton_method_name) @singleton_method_node

; Class declaration — class Foo ... end / class Foo < Bar ... end
(class
  name: (constant) @class_name) @class_node

; Module declaration — module Foo ... end
(module
  name: (constant) @module_name) @module_node

; Constant assignment — FOO = 42
(assignment
  left: (constant) @const_name
  right: (_)) @const_node
