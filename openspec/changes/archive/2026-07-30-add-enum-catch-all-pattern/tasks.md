## 1. Explicit arm-pattern representation

- [x] 1.1 Add `MatchPattern::{Variant, CatchAll}`, move qualified-arm fields under `Variant`, and update AST comments and test helpers.
- [x] 1.2 Mechanically update loader, checker, requirement analyzer, and evaluator consumers for the `Variant` case while preserving all existing match behavior.
- [x] 1.3 Run formatting and the complete Rust test suite, then commit the verified representation-only snapshot.

## 2. Catch-all syntax and static semantics

- [x] 2.1 Parse a whole-arm `_` with both simple and block bodies, reject payload lists on it, and add focused parser tests without changing payload `_` patterns.
- [x] 2.2 Canonicalize only qualified variant patterns in module loading and cover catch-all traversal through imported modules.
- [x] 2.3 Extend match checking so `_` is unique, final, and sufficient for exhaustiveness while preserving qualified variant membership, duplication, payload, and span diagnostics.
- [x] 2.4 Cover missing variants with and without `_`, duplicate and non-final `_`, a redundant final `_`, empty enums, payload variants, and deterministic diagnostic spans.

## 3. Runtime, typing, and ambient analysis

- [x] 3.1 Select an exact qualified arm before falling back to `_`, evaluate the subject and selected body once, and run catch-all bodies without payload bindings.
- [x] 3.2 Include catch-all bodies in inferred and expected result-type checks, render `_` in arm diagnostics, and add compatible and incompatible result tests.
- [x] 3.3 Scan catch-all bodies conservatively for calls and ambient requirements without introducing a local binding, and add shadowing and non-selected-arm coverage.

## 4. Integration, documentation, and verification

- [x] 4.1 Add CLI fixtures for qualified-arm precedence, payload variants handled by `_`, catch-all type errors, and ambient use in a catch-all body.
- [x] 4.2 Update `docs/grammar.md`, `docs/overview.md`, `README.md`, and relevant module documentation with `_` syntax, exhaustiveness, no-binding semantics, and the remaining guard/nested-pattern boundary.
- [x] 4.3 Run formatting, the complete Rust test suite, canonical and missing-handler examples, and strict OpenSpec validation; fix all regressions and commit the verified implementation snapshot.
