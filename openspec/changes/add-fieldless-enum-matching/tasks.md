## 1. Syntax and resolved representation

- [x] 1.1 Add the `match` token, `ExprKind::Match`, `MatchArm`, and parsing for qualified arms with `: expression` or block bodies; cover valid forms, empty arms, and malformed arm paths with lexer/parser tests.
- [x] 1.2 Extend module traversal and canonical-name resolution across match subjects and arm bodies, resolve each arm's enum path, and add tests for local, imported, unknown, and cross-module variants.

## 2. Qualified variant values

- [x] 2.1 Recognize a declared `Enum::Variant` path as the existing enum value in type inference and evaluation while preserving associated-function and ambient path behavior.
- [x] 2.2 Add typechecker and evaluator tests for bare/qualified equality, local shadowing, unknown variants, non-enum qualifiers, and same-named variants in different enums or modules.

## 3. Match semantics and analysis

- [x] 3.1 Type-check match subjects as known non-optional enums and diagnose foreign, unknown, duplicate, and missing arms deterministically, including the empty-enum case.
- [x] 3.2 Check every arm against the surrounding expected type or the first inferable arm type, infer the match result for downstream checks, and add coverage for compatible, incompatible, optional-injection, and unknown-result cases.
- [x] 3.3 Evaluate the subject exactly once, execute only the matching arm under existing block/return semantics, and retain defensive runtime errors for unchecked invalid subjects or arm sets.
- [x] 3.4 Traverse the subject and every arm in requirement analysis with isolated lexical scopes, merge all possible arm requirements, and test direct and transitive ambient use.

## 4. Integration and documentation

- [ ] 4.1 Add CLI integration programs covering a successful value-producing match and representative parse, subject-type, exhaustiveness, arm-type, and unsatisfied-ambient diagnostics.
- [ ] 4.2 Update `docs/grammar.md`, `docs/overview.md`, and `README.md` to document qualified variants, value-producing exhaustive match, its deliberate fieldless boundary, and the resulting next roadmap gap.
- [ ] 4.3 Run formatting, the complete Rust test suite, canonical and missing-handler examples, and strict OpenSpec validation; fix all regressions before marking the change complete.
