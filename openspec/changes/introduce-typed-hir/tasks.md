## 1. HIR Vocabulary and Storage

- [x] 1.1 Add `src/hir.rs` with distinct program-local ID newtypes and checked arena allocation/access helpers
- [x] 1.2 Define HIR builtin, struct, enum, array, optional, and divergent result types without canonical-name strings in semantic type references
- [x] 1.3 Define owned declaration records for structs, fields, enums, variants, traits, trait methods, slots, tests, callables, and trait implementations
- [x] 1.4 Define the structured expression arena, `ExprId`, `LocalId`, source spans, blocks, heads, match arms, provisions, and resolved call variants
- [x] 1.5 Add deterministic debug rendering and focused tests proving IDs are declaration-ordered and cannot be mixed across declaration kinds
- [x] 1.6 Run formatting, the full test suite, and strict OpenSpec validation, then commit the HIR vocabulary as a stable snapshot

## 2. Declaration Lowering

- [x] 2.1 Refactor type-checker declaration collection to allocate deterministic HIR IDs while retaining canonical names and declaration spans for presentation
- [x] 2.2 Lower builtin, struct, field, enum, variant, and payload types to resolved HIR types
- [x] 2.3 Lower traits and trait method signatures, including receiver form and effective `unit` returns
- [x] 2.4 Lower top-level function and test shells with parameter locals, effective return types, and source declaration order
- [x] 2.5 Lower inherent and trait implementation shells, map trait methods to implementing callables, and retain existing conformance diagnostics
- [x] 2.6 Lower effects to `SlotId` plus `TraitId` and preserve duplicate-slot diagnostics
- [x] 2.7 Add declaration snapshots for the canonical and multi-module programs and prove equivalent canonical names receive one ID
- [x] 2.8 Run formatting, the full test suite, and strict OpenSpec validation, then commit declaration lowering as a stable snapshot

## 3. Typed Expression Lowering

- [x] 3.1 Allocate callable-local IDs for parameters, `self`, `let`, loop bindings, and match payload bindings while preserving current lexical scopes
- [x] 3.2 Lower literals, local references, zero-field structs, fieldless variants, unary expressions, and binary expressions with concrete result types
- [x] 3.3 Lower `let`, assignment, return, assert, blocks, and discarded intermediate expressions with `Value(Type)` or `Diverges` classification
- [x] 3.4 Lower struct literals, field reads, field assignments, arrays, optional field access, and fallback with resolved declaration and field IDs
- [x] 3.5 Lower loops, conditionals, `with`, and nested block heads while preserving lexical slot shadowing and expression result types
- [x] 3.6 Lower enum constructors and matches with `VariantId`, payload `LocalId`s, guard expressions, catch-all arms, and divergent-arm joins
- [x] 3.7 Lower direct, concrete method, concrete associated, slot value/type method, and enum-constructor calls to their resolved HIR call forms
- [x] 3.8 Lower value and type provisions to `SlotId` and the statically checked `TraitImplId`
- [x] 3.9 Add structural coverage for every accepted `ExprKind`, including executed and unexecuted declarations and dependency modules
- [x] 3.10 Run formatting, the full test suite, and strict OpenSpec validation, then commit expression lowering as a stable snapshot

## 4. Authoritative Check-and-Lower Boundary

- [x] 4.1 Change the authoritative type-checking traversal to emit HIR as it computes types and call targets instead of recovering facts in a second pass
- [x] 4.2 Expose `check_and_lower` as `Result<hir::Program, Vec<Diag>>` and discard all partial HIR when diagnostics exist
- [x] 4.3 Keep a temporary diagnostics-only compatibility wrapper for existing checker tests and migrate tests to the new result boundary in small groups
- [x] 4.4 Add invariants proving successful HIR contains no poisoned expression, unknown type, unresolved call, unresolved field, or unresolved provision
- [x] 4.5 Pin existing diagnostic text, ordering, labels, help, related spans, and cross-module source rendering across failed lowering
- [x] 4.6 Run formatting, the full test suite, and strict OpenSpec validation, then commit the authoritative HIR boundary as a stable snapshot

## 5. HIR Requirement Analysis

- [x] 5.1 Add HIR body-fact scanning keyed by callable, trait-method virtual body, slot, local, and resolved call IDs
- [x] 5.2 Merge trait implementation method facts into both concrete callable and trait-method virtual identities without string keys
- [x] 5.3 Port provision masking, slot type/value levels, fixed-point propagation, and call-path construction to HIR IDs
- [x] 5.4 Preserve public requirement ordering, canonical display names, rendered summaries, diagnostics, and related call-site spans
- [x] 5.5 Add differential tests comparing complete AST and HIR analysis results for canonical, recursive, multi-module, match, and nested-provider programs
- [x] 5.6 Switch CLI requirement analysis to HIR after differential coverage passes
- [x] 5.7 Remove AST requirement scanning and temporary string body keys while retaining the existing fixed-point algorithm
- [x] 5.8 Run formatting, the full test suite, and strict OpenSpec validation, then commit HIR requirement analysis as a stable snapshot

## 6. HIR Reference Interpreter

- [x] 6.1 Add a parallel HIR interpreter with environments keyed by `LocalId`, direct jumps by `CallableId`, and program-aware value rendering
- [x] 6.2 Move runtime struct and enum identity from canonical strings to `StructId` and `VariantId`
- [x] 6.3 Execute scalar operations, locals, assignment, blocks, return, assert, loops, and conditionals from resolved HIR expressions
- [x] 6.4 Execute structs, arrays, optionals, enum constructors, match payload bindings, guards, and shared mutation with existing runtime semantics
- [x] 6.5 Execute concrete methods and associated functions by direct `CallableId` without runtime candidate search
- [x] 6.6 Represent ambient type/value bindings with `TraitImplId` and dispatch slot calls through `TraitMethodId`
- [x] 6.7 Preserve nested provider selection, type-versus-value provision behavior, recursion, and runtime diagnostic spans
- [x] 6.8 Add differential tests comparing AST and HIR evaluation values, mutations, errors, output, and canonical test results
- [x] 6.9 Switch CLI execution and test running to the HIR interpreter after differential coverage passes
- [x] 6.10 Run formatting, the full test suite, and strict OpenSpec validation, then commit the HIR interpreter cutover as a stable snapshot

## 7. Remove the AST Semantic Pipeline

- [x] 7.1 Remove the AST interpreter, runtime source-name method lookup, and compatibility APIs superseded by HIR execution
- [x] 7.2 Remove AST requirement scanning and declaration indexes duplicated by the HIR program
- [x] 7.3 Rename or consolidate temporary HIR modules so the final pipeline has one type checker, one requirement analyzer, and one interpreter
- [ ] 7.4 Verify `main` follows `load AST → check/lower HIR → analyze HIR → eval HIR` with no semantic consumer returning to AST
- [ ] 7.5 Update `README.md`, `docs/overview.md`, file maps, and `docs/compiler-roadmap.md` to describe the HIR boundary and mark this phase complete
- [ ] 7.6 Run canonical CLI success and failure examples, formatting, the full test suite, and strict OpenSpec validation
- [ ] 7.7 Commit the completed HIR migration as a stable snapshot ready for the ambient runtime ABI change
