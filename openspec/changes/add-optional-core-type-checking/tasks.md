## 1. Contextual Nil Compatibility

- [x] 1.1 Add type-checker tests for `nil` in optional and non-optional arguments, returns, struct fields, field assignments, and typed local reassignments
- [x] 1.2 Centralize expected-type compatibility so contextual `nil` is accepted only for `T?` across every existing use site
- [x] 1.3 Add and implement equality tests for optional-versus-`nil`, non-optional-versus-`nil`, and `nil == nil`
- [x] 1.4 Verify focused and full tests and commit the stable contextual-`nil` snapshot

## 2. Fallback Typing

- [ ] 2.1 Add type-checker tests for valid `T? ?? T`, a non-optional left operand, incompatible and optional fallback values, literal `nil ?? T`, and an unknown left operand
- [ ] 2.2 Validate known fallback operands and infer non-optional `T` for a valid-shape `??` expression without adding inference side effects
- [ ] 2.3 Add and implement tests for a direct `return` fallback, including ordinary validation of the returned value
- [ ] 2.4 Verify fallback result types reach argument, assignment, equality, and function-return checks
- [ ] 2.5 Verify focused and full tests and commit the stable fallback-typing snapshot

## 3. Integration and Documentation

- [ ] 3.1 Add CLI integration coverage for invalid contextual `nil`, invalid fallback operands, and fallback-result propagation
- [ ] 3.2 Update `README.md`, `docs/overview.md`, `docs/grammar.md`, and `src/typecheck.rs` boundary documentation, keeping optional field access explicitly deferred
- [ ] 3.3 Run `cargo fmt --check`, the full Rust test suite, the canonical example, and the missing-handler example
- [ ] 3.4 Validate the OpenSpec change, review the final diff, and commit the completed implementation snapshot
