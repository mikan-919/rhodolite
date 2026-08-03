## 1. Callable syntax and checked HIR

- [x] 1.1 Extend AST types, lexer/parser coverage, and grammar documentation for concrete `fn(P -> R)` callable annotations without changing existing function declarations.
- [x] 1.2 Lower named free-function references and callable-local invocations into typed HIR callable-value and indirect-call forms; update dumps and poison validation.
- [x] 1.3 Add focused parser and HIR tests for callable type spelling, inferred and annotated immutable aliases, and unchanged direct-call syntax.

## 2. Static checking and ownership boundary

- [x] 2.1 Type-check exact callable signature compatibility for free-function values, callback arguments, and indirect-call arguments/results with source-positioned diagnostics.
- [x] 2.2 Reject closures, method/associated/slot values, callable return types, mutable callable locals, and callable aggregate storage before ownership checking.
- [x] 2.3 Teach ownership checking that callable values are Copy while preserving existing move, borrow, and cleanup behavior for all other values; add focused tests.

## 3. Callback-specialized requirements and planning

- [x] 3.1 Define canonical callback binding contexts and specialization keys for callable parameters and immutable aliases; add deterministic interning and recursion tests.
- [x] 3.2 Compute exact ambient requirements and diagnostic paths per callback specialization while retaining deterministic human-facing requirement summaries.
- [x] 3.3 Extend ambient ABI planning so direct calls bind callback arguments, indirect calls resolve to their selected callable instance, and provider projections stay specialization-specific.
- [x] 3.4 Add requirement and planning tests for no-slot and slot-using callbacks, missing-provider paths, nested `with`, and callback-specialization deduplication.

## 4. Execution and Core Wasm lowering

- [x] 4.1 Represent and execute checked callable identities in the interpreter, including copied aliases and callback parameters.
- [x] 4.2 Lower planned indirect calls to deterministic direct Wasm calls without tables, `funcref`, closure allocation, host imports, or public ABI changes.
- [x] 4.3 Add focused interpreter/Wasm tests for callback return values, hidden ambient records, and byte-identical rebuilds.

## 5. End-to-end verification and documentation

- [x] 5.1 Add maintained differential fixtures for scalar callbacks and callbacks requiring ambient slots, plus intentional snapshot updates for representative Wasm modules.
- [x] 5.2 Update README, overview, grammar, and compiler roadmap to distinguish this named-function-value slice from future closures, generics, and `map`.
- [x] 5.3 Run `cargo fmt --check`, focused tests, the full Rust suite, strict Clippy, strict OpenSpec validation, and differential byte-determinism checks; review the final diff.
