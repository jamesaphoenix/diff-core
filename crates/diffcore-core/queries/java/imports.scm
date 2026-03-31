; Java import patterns
; Java uses `import pkg.Class;` and `import static pkg.Class.method;`

; Pattern 0: Regular import — import com.example.Foo;
(import_declaration
  (scoped_identifier) @source) @stmt

; Pattern 1: Static import — import static com.example.Foo.bar;
(import_declaration
  "static"
  (scoped_identifier) @static_source) @stmt

; Pattern 2: Wildcard import — import com.example.*;
; Captures both the scoped_identifier (package path) and the asterisk
(import_declaration
  (scoped_identifier) @wildcard_source
  (asterisk) @wildcard) @stmt
