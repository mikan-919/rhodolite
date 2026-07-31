## 1. Parse the Ownership Surface

- [x] 1.1 Add lexer tokens and source spans for `&`, `mut`, `move`, and `indirect`
  without changing the meaning of currently valid programs
- [x] 1.2 Extend AST types with owned/shared/mutable modes, signatures with
  `self`/`&self`/`&mut self`, bindings with `let mut`, and fields/payloads with
  `indirect`
- [x] 1.3 Parse `&T`, `&mut T`, `let mut`, ownership-qualified values, calls,
  receivers, `match`, `for`, `??`, and `with` provisions with the agreed
  place/receiver precedence
- [x] 1.4 Reject lifetime spellings, pattern-level moves, malformed ownership
  modifiers, user destructor/finalizer forms, raw pointers, and `unsafe` with
  source-positioned parser diagnostics
- [x] 1.5 Update AST dumps and parser tests for every new form, multiline and
  postfix interaction, parentheses equivalence, and every retained old grammar
  form
- [x] 1.6 Run formatting, the full existing test suite, focused parser fixtures,
  and strict OpenSpec validation, then commit the syntax surface as a stable
  snapshot

## 2. Resolve Owned and Reference Types

- [x] 2.1 Replace the HIR `has_self` Boolean with a resolved receiver mode and
  carry reference kinds, binding mutability, ownership modifiers, and indirect
  declaration edges through type lowering
- [x] 2.2 Extend type display, equality, destination compatibility, expected-type
  propagation, module-qualified annotations, diagnostics, and deterministic HIR
  dumps for `T`, `&T`, and `&mut T`
- [x] 2.3 Reject optional references and references nested in struct fields, enum
  payloads, optionals, or arrays while allowing reference locals, parameters,
  results, and projections
- [x] 2.4 Implement the initial Copy classification for scalar values, fieldless
  enums, shared references, and non-copyable mutable references, keeping all
  owned compound values non-Copy by default
- [x] 2.5 Build the owned type-containment graph, validate every recursive cycle
  has an `indirect` edge, and add direct, mutual, optional, array, enum, and valid
  indirect-cycle diagnostics with related spans
- [x] 2.6 Add resolved declaration tests for owned arrays/strings/structs/enums/
  optionals, receiver conformance, nested-reference rejection, Copy status, and
  indirect identities across modules
- [x] 2.7 Run formatting, the full test suite, HIR snapshots, type-cycle fixtures,
  and strict OpenSpec validation, then commit resolved ownership types as a
  stable snapshot

## 3. Establish the Ownership Plan and Move Analysis

- [ ] 3.1 Add `ownership::CheckedProgram`, deterministic body plans, `ScopeId`,
  CFG points/edges, places and projections, classified accesses, and drop-action
  data without exposing unchecked HIR to new normal pipeline entry points
- [ ] 3.2 Derive scopes from the existing Rhodolite lexical rules, proving that
  second-class blocks reuse their surrounding scope while arms, loops,
  provisions, functions, and tests retain their existing isolation
- [ ] 3.3 Implement path-sensitive initialization and move dataflow across
  sequences, branches, diverging paths, returns, and loop fixed points
- [ ] 3.4 Enforce immutable `let`, mutable `let mut`, Copy rebinding, default
  non-Copy local moves, explicit owned-call moves, fresh temporaries, and
  automatic owned returns
- [ ] 3.5 Implement whole-struct consuming field projection without partial
  moves, including dropping the remaining fields and rejecting every later use
  of the consumed root
- [ ] 3.6 Add diagnostics and related spans for use after move, maybe-moved branch
  joins, loop-carried moves, missing call-site `move`, reassignment of immutable
  bindings, and move from borrowed places
- [ ] 3.7 Add deterministic plan snapshots and compile-fail/pass tests for forward
  control flow, nested scopes, recursion, branch joins, loop backedges, Copy
  values, struct fields, and implicit return moves
- [ ] 3.8 Run formatting, the full test suite, ownership-plan snapshots, focused
  move diagnostics, and strict OpenSpec validation, then commit move analysis as
  a stable snapshot

## 4. Infer Loans and Return Provenance

- [ ] 4.1 Generate explicit and automatic shared/mutable loans, reborrow
  constraints, last-use constraints, and minimal non-lexical regions over CFG
  points
- [ ] 4.2 Implement place overlap for whole values and projections, accepting
  disjoint struct fields and conservatively overlapping enum payloads and
  unknown array indices
- [ ] 4.3 Reject mutation through shared access, overlapping mutable/shared
  loans, owner moves or drops during loans, mutable borrows from immutable
  places, and references that outlive their owners
- [ ] 4.4 Infer shared and mutable return provenance as deterministic sets of
  input paths and substitute caller places through direct and resolved method
  calls
- [ ] 4.5 Close recursive and mutually recursive return-provenance summaries with
  a monotone deterministic fixed point and conservatively union conditional
  origins
- [ ] 4.6 Reject borrowed values stored in aggregates or exposed in public Wasm
  signatures while retaining local, parameter, result, field-reborrow, and
  element-reborrow cases
- [ ] 4.7 Add plan snapshots and diagnostics for last-use shortening, reborrows,
  disjoint fields, conservative array conflicts, escaping returns, multiple
  return origins, recursion, and mutable returned loans
- [ ] 4.8 Run formatting, the full test suite, region/provenance fixed-point tests,
  determinism checks, and strict OpenSpec validation, then commit borrow
  inference as a stable snapshot

## 5. Integrate Calls, Receivers, and Providers

- [ ] 5.1 Enforce owned, shared, and mutable direct-function parameters after
  ordinary arity/type resolution, inserting only shared auto-borrows and keeping
  mutation/consumption visible
- [ ] 5.2 Enforce `&self`, `&mut self`, and consuming `self` on inherent and trait
  methods, exact receiver-mode conformance, and the receiver-qualified postfix
  call syntax
- [ ] 5.3 Propagate access mode and borrowed-result provenance through direct,
  associated, concrete-method, trait-method, and slot-call HIR without repeating
  candidate lookup
- [ ] 5.4 Add shared, mutable, moved, temporary-owned, and type-only modes to
  `with` providers; constrain their lifetime to the provision body and preserve
  nested provider shadowing
- [ ] 5.5 Update requirement and ambient planning inputs to consume checked HIR
  and provider modes while preserving callable/slot identities and every
  ownership-independent canonical plan fact
- [ ] 5.6 Add cross-module and recursive tests for each parameter/receiver mode,
  temporary arguments, missing modifiers, returned borrows, provider conflicts,
  trait conformance, and source-aware related diagnostics
- [ ] 5.7 Run formatting, the full test suite, requirement/ambient plan snapshots,
  call/provider failure fixtures, and strict OpenSpec validation, then commit
  ownership-aware calls and provisions as a stable snapshot

## 6. Integrate Owned Data Expressions

- [ ] 6.1 Implement owned struct construction, Copy/shared/mutable/consuming field
  projection, exclusive field replacement, and recursive ownership of direct and
  indirect fields in the plan
- [ ] 6.2 Implement owned optional injection and nil, Copy/borrowed/consuming `??`,
  lazy fallback movement, and ownership-safe optional-field projection
- [ ] 6.3 Implement owned payload enum construction plus shared, mutable, and
  consuming whole-scrutinee `match`, dropping unselected contents without
  partial-move states
- [ ] 6.4 Implement owned array construction plus shared, mutable, and consuming
  `for`, iteration-scoped element access, buffer ownership, and conflicting
  structural-mutation rejection
- [ ] 6.5 Implement compiler-known structural equality through shared borrows and
  explicit deep `clone()` for structs, active enum payloads, present optionals,
  strings, arrays, and indirect values; reject non-cloneable mutable references
- [ ] 6.6 Complete reverse-declaration drop plans for fallthrough, branch and loop
  exits, provision exits, explicit/final returns, moved values, and recursive
  composite contents while retaining trap-without-unwind behavior
- [ ] 6.7 Add focused ownership-plan and diagnostic fixtures for every data form,
  evaluation order, short-circuiting, mutation, deep clone independence,
  structural equality, consuming extraction, and exact drop-once behavior
- [ ] 6.8 Run formatting, the full test suite, data-expression plan snapshots,
  drop-path coverage, and strict OpenSpec validation, then commit complete owned
  expression planning as a stable snapshot

## 7. Make the Interpreter the Owned Reference Implementation

- [ ] 7.1 Replace semantic compound-value aliasing with an interpreter-owned
  location store, owned environment slots, statically checked reference places,
  and debug assertions for impossible ownership-plan violations
- [ ] 7.2 Evaluate Copy, move, shared borrow, mutable borrow, reborrow, owned
  return, field replacement, and consuming projection from the ownership plan
  without runtime borrow counters
- [ ] 7.3 Evaluate owned strings, arrays, structs, payload enums, optionals,
  indirect values, all three `match`/`for` modes, and ownership-aware provider
  scopes with preserved left-to-right order
- [ ] 7.4 Implement deep clone, structural equality, reverse-order recursive drop,
  early-return cleanup, move suppression, and execution-context disposal after
  runtime failure
- [ ] 7.5 Add interpreter regressions proving no implicit aliases, borrow-based
  mutation visibility, clone independence, returned-reference behavior,
  recursive indirect clone/drop, and unchanged successful scalar results
- [ ] 7.6 Run formatting, the full unit suite, focused interpreter success/failure
  corpus, leak/drop instrumentation tests, and strict OpenSpec validation, then
  commit the owned interpreter as a stable snapshot

## 8. Cut Over Repository Sources and the Build Pipeline

- [ ] 8.1 Route CLI interpretation, tests, requirement analysis, production
  planning, and Wasm building through `ownership::CheckedProgram`, removing every
  normal path that accepts ownership-unchecked HIR
- [ ] 8.2 Migrate the canonical program, examples, inline fixtures, parser/HIR
  snapshots, and CLI corpus to `let mut`, receiver/parameter modes, visible
  `&mut`, visible call-site `move`, and ownership-aware `with`
- [ ] 8.3 Replace old implicit-alias tests with explicit move, borrow, mutable
  access, clone, drop, and rejection cases; retain no compatibility flag or
  legacy evaluator behavior
- [ ] 8.4 Keep reachable non-scalar/reference code unsupported in Core Wasm,
  reject public borrowed signatures, and prove the existing scalar Wasm corpus
  remains valid, executable, deterministic, import-free, and start-free
- [ ] 8.5 Preserve whole-program checking of ownership errors in unreachable code
  and reachability-sensitive backend support for ownership-safe unsupported data
- [ ] 8.6 Run formatting, all unit and CLI tests, the migrated canonical program,
  missing-provider/runtime diagnostics, ambient-plan snapshots, Wasm validation/
  execution, deterministic rebuild comparison, and strict OpenSpec validation,
  then commit the repository-wide ownership cutover as a stable snapshot

## 9. Record the Ownership Contract

- [ ] 9.1 Add an ADR for owned values, implicit shared calls, explicit `&mut`/
  `move`/`clone`/`indirect`, whole-program lifetime inference, deterministic
  drop, and the explicit rejection of GC, runtime borrow checks, and `unsafe`
- [ ] 9.2 Update `docs/grammar.md`, `README.md`, and `docs/overview.md` with the
  exact syntax, examples, scoping caveat for second-class blocks, diagnostics,
  and interpreter/Wasm support boundary
- [ ] 9.3 Update `docs/compiler-roadmap.md` to place
  `introduce-ownership-and-borrowing` before
  `compile-wasm-owned-data-values`, and record aggregate borrows, explicit shared
  ownership, allocator/data layout, async, and rich ABI work as later stages
- [ ] 9.4 Run `cargo fmt --check`, the full test suite, canonical interpreter and
  CLI cases, ownership compile-pass/fail corpus, plan determinism, scalar Wasm
  engine fixtures, and `bunx @fission-ai/openspec validate --all --strict`
- [ ] 9.5 Review the complete diff for accidental allocator, WasmGC, RC/GC,
  `unsafe`, raw pointer, partial-move, aggregate-borrow, rich-ABI, or legacy-mode
  scope and resolve every warning introduced by this change
- [ ] 9.6 Commit the completed ownership implementation and documentation as an
  archive-ready stable snapshot
