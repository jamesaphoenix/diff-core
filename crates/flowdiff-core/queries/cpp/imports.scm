; C++ #include patterns
; C++ uses #include "file.h" and #include <header>
;
; AST structure:
;   (preproc_include
;     path: (string_literal))           — #include "local.h"
;   (preproc_include
;     path: (system_lib_string))        — #include <iostream>

; Pattern 0: Local include — #include "myheader.h"
(preproc_include
  path: (string_literal) @source) @stmt

; Pattern 1: System include — #include <iostream>
(preproc_include
  path: (system_lib_string) @source) @stmt
