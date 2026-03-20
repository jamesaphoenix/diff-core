; PHP import patterns
; PHP uses `use Namespace\Class;` for namespace imports and
; `require`/`include` for file inclusion.

; Pattern 0: Regular use — use App\Models\User;
(namespace_use_declaration
  (namespace_use_clause
    (qualified_name) @source)) @stmt

; Pattern 1: Use with alias — use App\Services\UserService as US;
; The alias `name` node is a sibling of `qualified_name` inside `namespace_use_clause`
(namespace_use_declaration
  (namespace_use_clause
    (qualified_name) @source
    (name) @alias)) @stmt

; Pattern 2: Require — require 'path/file.php';
(expression_statement
  (require_expression
    (string
      (string_content) @include_source))) @stmt

; Pattern 3: Require once — require_once 'path/file.php';
(expression_statement
  (require_once_expression
    (string
      (string_content) @include_source))) @stmt

; Pattern 4: Include — include 'path/file.php';
(expression_statement
  (include_expression
    (string
      (string_content) @include_source))) @stmt

; Pattern 5: Include once — include_once 'path/file.php';
(expression_statement
  (include_once_expression
    (string
      (string_content) @include_source))) @stmt
