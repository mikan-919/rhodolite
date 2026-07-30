## 1. Guard representation and syntax

- [x] 1.1 Add `MatchArm::guard: Option<Box<Expr>>`, update every existing arm constructor with `None`, and keep the complete Rust test suite green.
- [x] 1.2 Parse `Enum::Variant(payload) if condition` with simple and block arm bodies, reject `if` after `_`, and add focused parser tests for valid and malformed forms.
- [x] 1.3 Resolve names and collect paths in guards under the same payload-local scope as their arm bodies, with tests for imported names and payload shadowing.
- [x] 1.4 Run formatting and the complete Rust test suite, then commit the verified representation, syntax, and loading snapshot.

## 2. Static guard semantics

- [x] 2.1 Check each guard expression under payload locals and require inferable guard types to be `bool`, preserving the existing deferral boundary for unknown types.
- [x] 2.2 Track guarded and unconditional variant coverage separately so guarded arms require a final `_`, while keeping duplicate-variant and catch-all rules unchanged.
- [x] 2.3 Add checker tests for boolean, known non-boolean, and unknown guards; guarded exhaustiveness; duplicate guarded variants; payload types; result typing; and guard diagnostic spans.
- [x] 2.4 Run formatting and the complete Rust test suite, then commit the verified static-semantics snapshot.

## 3. Requirement analysis and evaluation

- [x] 3.1 Scan every guard conservatively for calls and ambient requirements under payload locals without leaking guard or body bindings, and add shadowing coverage.
- [x] 3.2 Evaluate a matching qualified arm's guard exactly once after binding its payload, select its body only on `true`, and fall back to `_` on `false`.
- [x] 3.3 Preserve runtime safety nets for non-boolean guards and missing fallback arms, with diagnostics pointing to the guard expression and tests pinning subject, guard, and body evaluation counts.
- [x] 3.4 Add CLI coverage for a true guard, false-guard fallback, payload use in a guard, a static guard type error, and an ambient requirement originating in a guard.
- [x] 3.5 Run formatting and the complete Rust test suite, then commit the verified runtime and analysis snapshot.

## 4. Documentation and final verification

- [ ] 4.1 Update `docs/grammar.md`, `docs/overview.md`, `README.md` where relevant, and AST/compiler comments with guard syntax, scope, boolean checking, fallback, exhaustiveness, and remaining pattern boundaries.
- [ ] 4.2 Run `cargo fmt --check`, the complete Rust test suite, canonical and missing-handler examples, and strict OpenSpec validation; fix all regressions.
- [ ] 4.3 Commit the final verified implementation and documentation snapshot.
