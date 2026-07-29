## 1. Syntax and declared representation

- [ ] 1.1 Replace enum variant names with `EnumVariant { name, payload }`, add flat `PatternBinding` values to `MatchArm`, and update existing fieldless fixtures to use the zero-payload representation.
- [ ] 1.2 Parse positional payload types in enum declarations and identifier / `_` payload patterns in qualified match arms, with tests for mixed variants, multiple payloads, malformed lists, and unchanged fieldless syntax.

## 2. Module resolution and traversal

- [ ] 2.1 Resolve every enum payload type through the existing canonical nominal-type rules and add local, imported, optional / array, and cross-module tests.
- [ ] 2.2 Update expression and item traversal for payload declarations and arm bindings, preserving canonical arm enum paths and arm-local shadowing without leaking names to sibling arms or following expressions.

## 3. Static construction and pattern checking

- [ ] 3.1 Recognize `Call(Path([enum, variant]), args)` as a qualified payload constructor before associated-function resolution, infer its enum result type, and diagnose wrong arity, wrong known argument types, and a payload variant path used without construction.
- [ ] 3.2 Validate each match pattern against its declared variant payload, diagnose arity and duplicate-binding errors, and type each named binding for field, call, assignment, and return checks.
- [ ] 3.3 Add regression tests for fieldless variants, non-variant path calls, constructor resolution precedence, optional injection at payload arguments, and deterministic span-bearing diagnostics.

## 4. Runtime values and ambient analysis

- [ ] 4.1 Extend `Value::Enum` with ordered payload values, construct arguments exactly once, include payloads in display and recursive equality, and cover equal, unequal, multi-payload, and shared composite payload cases.
- [ ] 4.2 Bind the selected arm's payload values in an arm-local runtime environment, discard `_`, execute only the matching arm, and retain defensive errors for unchecked arity mismatches.
- [ ] 4.3 Teach requirement analysis that named pattern payloads shadow ambient slots only inside their arm while all possible arm bodies still contribute requirements.

## 5. Integration, documentation, and verification

- [ ] 5.1 Add CLI programs covering successful construction and destructuring, multi-payload order, `_`, fieldless compatibility, constructor errors, pattern errors, type flow, and an ambient slot shadowed by a payload binding.
- [ ] 5.2 Update `docs/grammar.md`, `docs/overview.md`, `README.md`, and relevant module documentation with payload syntax, static guarantees, structural equality, and the deferred guard / catch-all / nested-pattern boundary.
- [ ] 5.3 Run formatting, the complete Rust test suite, canonical and missing-handler examples, and strict OpenSpec validation; fix all regressions and commit the verified implementation snapshot.
