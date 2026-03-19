; TypeScript/JavaScript export patterns
; Pattern index determines the export variant.

; Pattern 0: Named export specifiers — export { foo, bar }
(export_statement
  (export_clause
    (export_specifier
      name: (_) @export_name))) @stmt

; Pattern 1: Re-export specifiers — export { baz } from './other'
(export_statement
  (export_clause
    (export_specifier
      name: (_) @reexport_name))
  source: (string
    (string_fragment) @reexport_source)) @stmt

; Pattern 2: Re-export with source but no export_clause (wildcard: export * from './other')
; Also matches namespace exports like export * as ns from './other'
; Engine deduplicates with pattern 1 (re-exports with export_clause)
(export_statement
  source: (string
    (string_fragment) @wildcard_source)) @stmt

; Pattern 3: Exported function declaration — export function foo() {}
(export_statement
  declaration: (function_declaration
    name: (identifier) @decl_fn_name)) @stmt

; Pattern 4: Exported generator function — export function* gen() {}
(export_statement
  declaration: (generator_function_declaration
    name: (identifier) @decl_gen_name)) @stmt

; Pattern 5: Exported class — export class Foo {}
(export_statement
  declaration: (class_declaration
    name: (_) @decl_class_name)) @stmt

; Pattern 6: Exported abstract class — export abstract class Foo {}
(export_statement
  declaration: (abstract_class_declaration
    name: (_) @decl_abstract_name)) @stmt

; Pattern 7: Exported interface — export interface IFoo {}
(export_statement
  declaration: (interface_declaration
    name: (_) @decl_iface_name)) @stmt

; Pattern 8: Exported type alias — export type Foo = ...
(export_statement
  declaration: (type_alias_declaration
    name: (_) @decl_type_name)) @stmt

; Pattern 9: Exported variable — export const VALUE = 42
(export_statement
  declaration: (lexical_declaration
    (variable_declarator
      name: (identifier) @decl_var_name))) @stmt
