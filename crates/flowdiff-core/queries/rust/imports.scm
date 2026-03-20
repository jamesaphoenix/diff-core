; Rust import (use) patterns
; Rust uses `use` statements to bring paths into scope.
; The tree-sitter-rust grammar has `use_declaration` nodes.

; Pattern 0: Simple use — use std::io;
(use_declaration
  argument: (scoped_identifier) @source) @stmt

; Pattern 1: Use with alias — use std::io as stdio;
(use_declaration
  argument: (use_as_clause
    path: (scoped_identifier) @source
    alias: (identifier) @alias_name)) @stmt

; Pattern 2: Use with glob — use std::io::*;
(use_declaration
  argument: (use_wildcard
    (scoped_identifier) @source)) @stmt

; Pattern 3: Use list — use std::io::{Read, Write};
(use_declaration
  argument: (scoped_use_list
    path: (scoped_identifier) @source
    list: (use_list
      (identifier) @named_name))) @stmt

; Pattern 4: Use list with aliases — use std::io::{Read as R};
(use_declaration
  argument: (scoped_use_list
    path: (scoped_identifier) @source
    list: (use_list
      (use_as_clause
        path: (identifier) @aliased_name
        alias: (identifier) @alias)))) @stmt

; Pattern 5: Use list from crate/self — use crate::module::{Foo, Bar};
(use_declaration
  argument: (scoped_use_list
    path: (identifier) @source
    list: (use_list
      (identifier) @named_name))) @stmt

; Pattern 6: Simple identifier use — use serde;
(use_declaration
  argument: (identifier) @source) @stmt

; Pattern 7: Use list with nested scoped identifiers — use std::collections::{HashMap, BTreeMap};
(use_declaration
  argument: (scoped_use_list
    path: (scoped_identifier) @source
    list: (use_list
      (scoped_identifier) @named_name))) @stmt

; Pattern 8: Use list from identifier with scoped items — use crate::{module::Foo};
(use_declaration
  argument: (scoped_use_list
    path: (identifier) @source
    list: (use_list
      (scoped_identifier) @named_name))) @stmt
