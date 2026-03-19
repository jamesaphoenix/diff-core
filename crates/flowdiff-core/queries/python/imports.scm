; Python import patterns
; Pattern index determines the import variant.

; Pattern 0: Simple import — import foo
(import_statement
  name: (dotted_name) @module_name) @stmt

; Pattern 1: Aliased import — import foo as bar
(import_statement
  name: (aliased_import
    name: (dotted_name) @module_name
    alias: (identifier) @alias)) @stmt

; Pattern 2: From import (named) — from foo import bar
(import_from_statement
  module_name: (dotted_name) @source
  name: (dotted_name) @imported_name) @stmt

; Pattern 3: From import with alias — from foo import bar as baz
(import_from_statement
  module_name: (dotted_name) @source
  name: (aliased_import
    name: (dotted_name) @aliased_imported_name
    alias: (identifier) @imported_alias)) @stmt

; Pattern 4: From import wildcard — from foo import *
(import_from_statement
  module_name: (dotted_name) @source
  (wildcard_import) @wildcard) @stmt

; Pattern 5: Relative import — from . import foo, from .bar import baz
(import_from_statement
  module_name: (relative_import) @relative_source
  name: (dotted_name) @relative_imported_name) @stmt
