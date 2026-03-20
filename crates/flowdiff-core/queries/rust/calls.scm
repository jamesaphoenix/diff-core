; Rust call expression patterns

; Pattern 0: Any call expression — foo(), Bar::new(), self.method()
(call_expression
  function: (_) @callee
  arguments: (arguments) @args) @node
