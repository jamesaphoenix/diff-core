; C# using directive patterns
; C# uses `using Namespace;` and `using static Namespace.Type;`

; Pattern 0: Using with qualified name — using System.Collections.Generic;
(using_directive
  (qualified_name) @source) @stmt

; Pattern 1: Using with simple identifier — using System;
(using_directive
  (identifier) @source) @stmt
