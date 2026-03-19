; TypeScript/JavaScript import patterns
; Each pattern captures @source (import path) and @stmt (import_statement node).
; Pattern index determines the import variant.

; Pattern 0: Default import — import Foo from 'bar'
(import_statement
  (import_clause
    (identifier) @default_name)
  source: (string
    (string_fragment) @source)) @stmt

; Pattern 1: Named import specifier — import { a, b } from 'bar'
; Note: also matches specifiers with aliases; engine deduplicates.
(import_statement
  (import_clause
    (named_imports
      (import_specifier
        name: (identifier) @named_name)))
  source: (string
    (string_fragment) @source)) @stmt

; Pattern 2: Named import with alias — import { foo as bar } from 'baz'
(import_statement
  (import_clause
    (named_imports
      (import_specifier
        name: (identifier) @aliased_name
        alias: (identifier) @alias)))
  source: (string
    (string_fragment) @source)) @stmt

; Pattern 3: Namespace import — import * as ns from 'bar'
(import_statement
  (import_clause
    (namespace_import
      (identifier) @ns_name))
  source: (string
    (string_fragment) @source)) @stmt

; Pattern 4: Side-effect import — import './polyfill'
(import_statement
  source: (string
    (string_fragment) @source)) @stmt
