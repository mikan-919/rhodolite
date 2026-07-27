## 1. Syntax, Runtime, and Traversal

- [x] 1.1 Add the approved `ExprKind::OptionalField` variant and parser tests for `receiver.?field`, chaining, and continued dotted lines
- [x] 1.2 Reject `OptionalField` as an assignment target and as a call suffix with focused parser diagnostics
- [x] 1.3 Implement evaluator tests and behavior for present receivers, nil propagation, single receiver evaluation, and chained short-circuiting
- [x] 1.4 Add exhaustive module-rewrite and requirement-scan handling with regression tests for receiver traversal
- [x] 1.5 Run focused and full tests and commit the stable syntax/runtime snapshot

## 2. Static Optional Field Typing

- [x] 2.1 Add type-checker tests for valid `S?.?field`, flattened optional fields, missing fields, non-struct and non-optional receivers, and unknown receivers
- [x] 2.2 Validate known optional receivers and infer the declared field type with one optional bit
- [x] 2.3 Reject ordinary field access on known optional receivers, including after an optional projection
- [x] 2.4 Verify chained optional fields and propagation into fallback, arguments, assignments, equality, and returns
- [x] 2.5 Run focused and full tests and commit the stable type-checking snapshot

## 3. Integration and Documentation

- [ ] 3.1 Add CLI coverage for successful optional reads, nil propagation, invalid receiver forms, and inferred-result mismatches
- [ ] 3.2 Update `README.md`, `docs/overview.md`, `docs/grammar.md`, and checker boundary documentation with `.?` and the remaining unknown forms
- [ ] 3.3 Run `cargo fmt --check`, the full Rust suite, the canonical example, and the missing-handler example
- [ ] 3.4 Validate the OpenSpec change, review the final diff, and commit the completed implementation snapshot
