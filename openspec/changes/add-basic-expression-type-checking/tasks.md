## 1. Built-in Scalar Types

- [ ] 1.1 Add type-checker unit tests for `int`, `bool`, and `str` literal inference reaching direct-call and return checks
- [ ] 1.2 Reserve `int`, `bool`, `str`, and `unit` across loaded declaration kinds with deterministic redeclaration diagnostics
- [ ] 1.3 Implement built-in literal type facts while keeping `infer` free of diagnostic side effects
- [ ] 1.4 Migrate repository fixtures and the canonical program from `Int`, `Bool`, `Str`, `UserId`, and `Time` scalar spellings to `int`, `bool`, and `str`
- [ ] 1.5 Run the type-checker and CLI tests and commit the stable built-in-type migration snapshot

## 2. Ordinary Struct Field Typing

- [ ] 2.1 Add unit tests for valid and chained non-optional field reads, missing fields, known non-struct receivers, and temporarily unknown receivers
- [ ] 2.2 Validate known ordinary field receivers and declared field existence during expression traversal
- [ ] 2.3 Infer declared field types recursively without duplicating diagnostics
- [ ] 2.4 Run focused and full tests and commit the stable field-typing snapshot

## 3. Primitive Operators and Boolean Contexts

- [ ] 3.1 Add unit tests for integer arithmetic and negation, including rejected boolean and string operands
- [ ] 3.2 Implement `int × int -> int` arithmetic and `int -> int` negation checks and inference
- [ ] 3.3 Add unit tests for same-type equality, mismatched known operands, and a `bool` result with a temporarily unknown operand
- [ ] 3.4 Implement exact known-operand equality checks and infer every equality result as `bool`
- [ ] 3.5 Add and implement `bool` checks for inferable `if`, `elif`, `while`, and `assert` operands
- [ ] 3.6 Run focused and full tests and commit the stable operator-typing snapshot

## 4. Generalized Assignment Compatibility

- [ ] 4.1 Add unit tests for matching and mismatched scalar, struct, and enum values in struct literals and field assignments
- [ ] 4.2 Replace enum-only field compatibility with exact known-type comparison while preserving existing enum diagnostics
- [ ] 4.3 Add unit tests and checks for reassigning typed parameters and `let` bindings, including deferred unknown initializers
- [ ] 4.4 Run focused and full tests and commit the stable assignment-checking snapshot

## 5. Integration and Documentation

- [ ] 5.1 Add CLI integration coverage for reserved names, invalid fields, operator mismatches, non-boolean contexts, and assignment mismatches
- [ ] 5.2 Update `docs/overview.md`, `docs/grammar.md`, `README.md`, and `src/typecheck.rs` boundary documentation with the new guarantees and remaining temporary unknown forms
- [ ] 5.3 Run `cargo fmt --check`, the full Rust test suite, the canonical example, and the missing-handler example
- [ ] 5.4 Confirm no maintained Rhodolite source or type-system documentation still uses removed uppercase scalar spellings or undeclared integer aliases, then commit the final stable implementation snapshot
