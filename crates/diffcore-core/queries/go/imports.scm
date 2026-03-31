; Go import patterns
; Go imports use `import "path"` or `import ( "path1"\n "path2" )`
; Each import_spec has an optional alias (package_identifier) and a path (interpreted_string_literal).

; Pattern 0: Simple import — import "fmt"
(import_declaration
  (import_spec
    path: (interpreted_string_literal) @source)) @stmt

; Pattern 1: Aliased import — import alias "path"
(import_declaration
  (import_spec
    name: (package_identifier) @alias_name
    path: (interpreted_string_literal) @source)) @stmt

; Pattern 2: Dot import — import . "path" (imports all names into current scope)
(import_declaration
  (import_spec
    name: (dot) @dot_import
    path: (interpreted_string_literal) @source)) @stmt

; Pattern 3: Blank import — import _ "path" (side-effect only)
(import_declaration
  (import_spec
    name: (blank_identifier) @blank_import
    path: (interpreted_string_literal) @source)) @stmt

; Pattern 4: Import list — import ( "fmt"\n "net/http" )
(import_declaration
  (import_spec_list
    (import_spec
      path: (interpreted_string_literal) @source))) @stmt

; Pattern 5: Import list with alias — import ( alias "path" )
(import_declaration
  (import_spec_list
    (import_spec
      name: (package_identifier) @alias_name
      path: (interpreted_string_literal) @source))) @stmt

; Pattern 6: Import list with dot — import ( . "path" )
(import_declaration
  (import_spec_list
    (import_spec
      name: (dot) @dot_import
      path: (interpreted_string_literal) @source))) @stmt

; Pattern 7: Import list with blank — import ( _ "path" )
(import_declaration
  (import_spec_list
    (import_spec
      name: (blank_identifier) @blank_import
      path: (interpreted_string_literal) @source))) @stmt
