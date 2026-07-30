## 1. Local Binding Type Annotations

- [x] 1.1 Extend `ExprKind::Let` with an optional `Type`, parse `let name: Type = value`, and cover named, optional, array, and malformed annotations with parser tests
- [x] 1.2 Canonicalize named leaves inside local annotations in the module loader and add cross-module annotation tests
- [x] 1.3 Thread the annotation through requirement scanning and evaluation without changing runtime binding behavior
- [x] 1.4 Use an annotation as the initializer's expected type and the binding's fixed type, including positive and negative initializer and reassignment tests
- [x] 1.5 Run the full test suite and commit the annotation support as a stable snapshot

## 2. Effective Unit Returns

- [x] 2.1 Give every function, trait member, and implementation method an effective return type, using explicit annotations when present and `unit` otherwise
- [x] 2.2 Type direct, method, associated-function, and slot call results from their effective return types
- [x] 2.3 Check explicit returns and final body expressions against effective `unit` for declarations without a return annotation
- [x] 2.4 Migrate maintained sources and test fixtures that intentionally return non-`unit` values without annotations
- [x] 2.5 Add tests for omitted `unit` returns, invalid omitted non-`unit` returns, recursion, trait conformance, and call-result propagation
- [x] 2.6 Run the full test suite and commit effective return types as a stable snapshot

## 3. Total Checking Result

- [x] 3.1 Introduce internal `Typed`, `Diverges`, and `Poisoned` outcomes and preserve the current `KnownType` compatibility rules
- [x] 3.2 Replace optional compatibility helpers with directional synthesis/checking that can pass expected types into contextual expressions
- [x] 3.3 Classify `return`, empty blocks, ordinary blocks, discarded intermediate expressions, loops, conditionals, and `with` bodies without using Unknown for control-flow divergence
- [x] 3.4 Prevent cascaded diagnostics when a child is already poisoned while ensuring every unexplained untyped expression gets a positioned diagnostic
- [x] 3.5 Add focused tests for divergent branches, poisoned parents, unused invalid declarations, and diagnostics in dependency modules
- [x] 3.6 Run the full test suite and commit the total-checking foundation as a stable snapshot

## 4. Basic Expressions and Bindings

- [x] 4.1 Make every identifier, path, ordinary field read, and optional field read either synthesize a concrete type or produce a positioned diagnostic
- [x] 4.2 Require concrete operand types for arithmetic, negation, equality, conditions, and assertions
- [x] 4.3 Require concrete source and destination types for struct fields, field assignments, annotated initializers, and local reassignments
- [x] 4.4 Reject unannotated bindings whose initializer cannot synthesize a concrete type
- [x] 4.5 Replace tests that expected basic-expression deferral with success-or-diagnostic assertions and add CLI coverage
- [x] 4.6 Run the full test suite and commit closed basic expression checking as a stable snapshot

## 5. Calls and Signatures

- [x] 5.1 Make direct calls resolve to one loaded function and require every argument to have a compatible concrete type
- [x] 5.2 Make method and associated-function resolution reject untyped receivers, absent members, ambiguities, and receiver-form mismatches before evaluation
- [x] 5.3 Require every resolved method, associated-function, slot, and enum-constructor argument to have a compatible concrete type
- [x] 5.4 Add tests proving unresolved calls in unexecuted functions and dependency modules fail before evaluation
- [x] 5.5 Retain evaluator lookup defenses and add CLI tests proving checked programs do not rely on them
- [x] 5.6 Run the full test suite and commit complete call resolution as a stable snapshot

## 6. Arrays and Optionals

- [x] 6.1 Reject context-free empty arrays and arrays containing untyped elements while accepting empty arrays under annotated or destination context
- [x] 6.2 Require every `for` iterable to have a concrete non-optional array type and bind a concrete element type
- [x] 6.3 Contextualize `nil` only from a concrete optional destination, typed equality operand, or concrete fallback right operand
- [x] 6.4 Reject `let x = nil`, `nil == nil`, unresolved fallback operands, and unresolved optional-field receivers with positioned diagnostics
- [x] 6.5 Add positive annotation tests and replace deferred array and optional tests with total-checking expectations
- [x] 6.6 Run the full test suite and commit closed array and optional checking as a stable snapshot

## 7. Match, Heads, and Provisions

- [x] 7.1 Join all continuing match arm result types, propagate divergence, and reject a value-producing match with no concrete result type
- [x] 7.2 Classify an exhaustive zero-arm match over an empty enum as divergent and cover it in return and destination contexts
- [x] 7.3 Require every match guard to have concrete type `bool` before evaluation
- [x] 7.4 Require every conditional branch needed for a value to have a compatible concrete type while preserving `unit` for a conditional without `else`
- [x] 7.5 Require every `with slot(value)` expression to have a concrete type implementing the slot trait and remove runtime deferral from CLI behavior
- [x] 7.6 Replace deferred match, guard, enum-source, head, and provision tests with total-checking expectations
- [x] 7.7 Run the full test suite and commit closed composite-expression checking as a stable snapshot

## 8. Whole-Program Closure and Documentation

- [ ] 8.1 Add a whole-program audit that prevents successful checking when any loaded expression or call remains unclassified
- [ ] 8.2 Add integration fixtures covering all expression variants in both directly executed and unexecuted loaded declarations
- [ ] 8.3 Verify all pre-execution failures retain source spans and that successful canonical behavior and requirement rendering remain unchanged
- [ ] 8.4 Update `README.md`, `docs/overview.md`, and `docs/grammar.md` to replace partial inference and runtime deferral with the total-checking contract and local annotation syntax
- [ ] 8.5 Run `cargo fmt --check`, the full test suite, and strict OpenSpec validation
- [ ] 8.6 Commit the completed type-checking closure as a stable snapshot ready for HIR work
