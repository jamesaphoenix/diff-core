; Ruby import patterns
; Ruby uses `require`, `require_relative` for file imports,
; and `include`/`extend` for module mixins.

; Pattern 0: require 'json' / require 'active_record'
(call
  method: (identifier) @_method
  arguments: (argument_list
    (string
      (string_content) @source))
  (#eq? @_method "require")) @stmt

; Pattern 1: require_relative '../models/user'
(call
  method: (identifier) @_method
  arguments: (argument_list
    (string
      (string_content) @require_relative_source))
  (#eq? @_method "require_relative")) @stmt

; Pattern 2: include ModuleName (mixin)
(call
  method: (identifier) @_method
  arguments: (argument_list
    (constant) @include_name)
  (#eq? @_method "include")) @stmt

; Pattern 3: extend ModuleName (mixin)
(call
  method: (identifier) @_method
  arguments: (argument_list
    (constant) @include_name)
  (#eq? @_method "extend")) @stmt
