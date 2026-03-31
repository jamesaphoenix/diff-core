; Swift import patterns
; Swift uses `import Module`, `import struct Module.Type`
;
; AST structure:
;   (import_declaration
;     (identifier
;       (simple_identifier) ...))

; Pattern 0: Regular import — import Foundation
(import_declaration
  (identifier) @source) @stmt
