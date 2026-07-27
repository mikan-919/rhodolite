## 1. Declaration Contracts

- [x] 1.1 Extend the type-checker declaration index with trait signatures, slot-to-trait mappings, and concrete-type implementation candidates while preserving canonical module-resolved names.
- [x] 1.2 Validate trait `impl` targets, complete and unique method sets, receiver forms, parameter types, and return types, with focused diagnostics for every conformance failure.
- [x] 1.3 Add type-checker tests for matching implementations, missing/extra/duplicate methods, signature mismatches, and invalid trait or struct targets.

## 2. Static Call Resolution

- [x] 2.1 Refactor lexical bindings to distinguish typed or unknown local values from ambient slots, preserving local shadowing and the outer/inner scope behavior of `with slot(value)`.
- [x] 2.2 Implement deterministic resolution for concrete value methods, slot methods, concrete associated functions, and slot-associated functions, including absence, ambiguity, and receiver-form diagnostics.
- [x] 2.3 Add resolution tests covering unique inherent and trait candidates, same-named trait ambiguity, slot-trait disambiguation, unknown receivers, wrong call syntax, local slot shadowing, and `with db(db)`.

## 3. Signature Checking and Type Flow

- [x] 3.1 Route resolved calls through shared arity and directional argument-compatibility checking, distributing expected parameter types into nested argument expressions.
- [x] 3.2 Infer resolved call return types in bindings and downstream field, operator, optional fallback, assignment, argument, and return checks while leaving unannotated or unresolved results unknown.
- [x] 3.3 Add tests for argument counts and types, optional injection direction, return propagation, `db.find(id) ?? return false`, `clock.now()`, and associated constructors.

## 4. Integration and Documentation

- [x] 4.1 Run the full Rust test suite and the canonical program, correcting repository fixtures that contain signatures newly proven invalid without weakening the specified checks.
- [x] 4.2 Update `src/typecheck.rs` module documentation, `README.md`, `docs/overview.md`, and `docs/grammar.md` to describe statically resolved calls and identify provision compatibility as the next narrow type-checking boundary.
- [x] 4.3 Re-run formatting, all tests, canonical execution, and OpenSpec validation, then record the verified implementation as a Git snapshot.
