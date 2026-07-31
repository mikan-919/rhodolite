## 1. Close Integer Runtime Semantics

- [x] 1.1 Change interpreter addition, subtraction, multiplication, and negation to explicit signed
  64-bit wrapping operations, and change division to explicit checked signed division
- [x] 1.2 Add interpreter and CLI regressions for maximum/minimum wraparound, multiplication and
  negation wraparound, truncation toward zero, division by zero, and minimum divided by negative one
- [x] 1.3 Run formatting, the full test suite, canonical success and runtime-diagnostic cases, and
  strict OpenSpec validation, then commit integer semantics as a stable snapshot

## 2. Add Public Re-exports

- [x] 2.1 Extend lexing/parsing and AST dumps for top-level-prefix `pub use`, preserving every
  existing `use` path, selection, alias, multiline, and placement rule
- [x] 2.2 Build deterministic module public-member tables that preserve canonical declaration
  identity through aliases and transitive selected re-exports, including cyclic graph resolution
- [x] 2.3 Enforce the existing single namespace for public/local/import names and add
  source-positioned diagnostics for collisions, missing public members, and unresolved re-export
  cycles
- [x] 2.4 Carry all public item identities plus the entry module's explicitly selected public
  function names through loading and typed HIR lowering without recursively flattening public
  module namespaces
- [x] 2.5 Add parser, loader, HIR, and CLI tests for ordinary-use privacy, public functions and
  types, aliases, transitive re-exports, module re-exports, collisions, cycles, and explicit entry
  function selection
- [x] 2.6 Run formatting, the full test suite, canonical and multi-module CLI cases, and strict
  OpenSpec validation, then commit public re-exports as a stable snapshot

## 3. Generalize Specialization Roots

- [x] 3.1 Refactor the ambient planner to accept an ordered explicit root set and provider context
  while retaining a compatibility entry point for the existing `main + all tests` plan
- [x] 3.2 Generalize unsatisfied-requirement reporting to named production roots and preserve the
  existing main/test rendering, paths, labels, and related spans
- [x] 3.3 Build the production root set from `main` plus distinct entry-module public callables,
  start each from an empty provider context, and retain each ABI name-to-instance mapping
- [x] 3.4 Add tests for production/test separation, aliased roots sharing one instance,
  cross-root instance deduplication, closed and unclosed public requirements, deterministic root
  order, and an unchanged legacy canonical plan snapshot
- [x] 3.5 Run formatting, the full test suite, canonical and missing-provider CLI cases, and strict
  OpenSpec validation, then commit generalized root planning as a stable snapshot

## 4. Establish Core Wasm Emission

- [x] 4.1 Add narrowly scoped production dependencies for direct Core Wasm encoding, validation,
  and canonical JSON plus a development-only in-process Core Wasm execution engine
- [x] 4.2 Add a Wasm backend boundary that consumes typed HIR, the production specialization plan,
  the ABI export mapping, and validated signatures without repeating semantic resolution
- [x] 4.3 Assign deterministic function types, indices, and scalar locals for empty-record
  specialization instances, deduplicating shared instances and planned direct-call targets
- [x] 4.4 Add a reachability-sensitive support pass that accepts the v0 scalar subset and reports
  every reached unsupported type, expression, call form, or non-empty ambient layout at its HIR
  span
- [x] 4.5 Encode complete modules in memory, independently validate them, and add unit tests for
  valid sections, no imports/start section, reached versus unreachable unsupported code, stable
  indices, and byte-identical repeated emission
- [x] 4.6 Run formatting, the full test suite, focused validation/determinism tests, and strict
  OpenSpec validation, then commit the Core Wasm emission skeleton as a stable snapshot

## 5. Lower Scalar Expressions and Control Flow

- [x] 5.1 Lower unit/bool/int literals, scalar local reads, bindings, assignments, sequence drops,
  and block results with validation tests for unit erasure and stack balance
- [x] 5.2 Lower wrapping negation and arithmetic, signed division, and scalar equality with
  interpreter-versus-engine tests for ordinary, boundary, and trapping cases
- [x] 5.3 Lower typed `if` expressions and `while` loops with tests for value/unit branches,
  mutation, nested control flow, and evaluation order
- [x] 5.4 Lower planned direct calls, unit/scalar call results, direct return, recursion, mutual
  recursion, and `assert` traps without source-name lookup
- [x] 5.5 Add complete scalar fixtures combining locals, loops, branches, calls, recursion,
  returns, assertions, and all entry result types, and compare every successful result with the
  interpreter
- [x] 5.6 Run formatting, the full test suite, independent Wasm execution fixtures, and strict
  OpenSpec validation, then commit executable scalar lowering as a stable snapshot

## 6. Implement Rhodolite Wasm ABI v0

- [x] 6.1 Validate that `main` has no parameters and a unit/bool/int result, and that explicit
  public functions have only bool/int parameters and unit/bool/int results, with focused diagnostics
  for unit parameters, rich types, and reserved-name collisions
- [x] 6.2 Generate the `__rhodolite_main` wrapper and sorted public-name wrappers while keeping all
  specialized implementation functions private
- [x] 6.3 Implement strict zero-or-one Boolean argument checks, normalized Boolean results, direct
  signed `i64` transport, unit result erasure, and trap-only failure behavior
- [x] 6.4 Serialize the exact compact canonical ABI v0 JSON schema and embed exactly one
  `rhodolite.abi` custom section with stable key, parameter, and export ordering
- [x] 6.5 Add engine-level raw ABI tests for entry/public export names and visibility, aliases,
  every supported signature, invalid Boolean inputs, traps, shared implementation wrappers, and
  metadata discovery and byte equality
- [x] 6.6 Run formatting, the full test suite, ABI execution/metadata tests, and strict OpenSpec
  validation, then commit ABI v0 as a stable snapshot

## 7. Add the Wasm Build Command

- [x] 7.1 Introduce explicit CLI command parsing for
  `build <entry.rd> --target wasm [-o <output.wasm>]` while preserving no-argument and positional
  interpreter behavior byte-for-byte
- [x] 7.2 Connect build loading, whole-program checking, named-root requirement validation,
  production planning, support checking, emission, and binary validation through the existing
  source-aware diagnostic renderer
- [x] 7.3 Implement the invocation-relative `target/wasm/<entry-stem>.wasm` default, explicit
  output paths, parent-directory creation, and same-directory temporary publication without
  replacing an existing artifact on pre-publication failure
- [x] 7.4 Add CLI tests for default and explicit outputs, invalid target/options, load/type/
  requirement/support failures, unchanged outputs after failure, interpreter compatibility, and
  independently executable generated files
- [x] 7.5 Run formatting, the full unit and CLI suites, canonical interpreter success,
  missing-provider/runtime diagnostics, generated-module validation/execution, determinism, and
  strict OpenSpec validation, then commit the user-visible build path as a stable snapshot

## 8. Record and Verify the Backend Boundary

- [x] 8.1 Add an ADR for Core Wasm plus Rhodolite ABI v0, public re-exports, embedded metadata,
  trap transport, and Component/WIT as optional framework adapters
- [x] 8.2 Update `README.md`, `docs/overview.md`, and `docs/compiler-roadmap.md` from the C plan to
  the Wasm sequence, documenting the current scalar completion line and the next data/provider
  stages without rewriting archived changes
- [x] 8.3 Run `cargo fmt --check`, the full test suite, every maintained CLI success/failure case,
  independent Wasm validation and execution, deterministic rebuild comparison, and
  `bunx @fission-ai/openspec validate --all --strict`
- [x] 8.4 Review generated dependency lockfile changes and the complete diff for accidental
  Component/WIT, WASI, JavaScript, host-framework, rich-data, or non-empty-record scope
- [x] 8.5 Commit the completed Core Wasm build and documentation as an archive-ready stable
  snapshot
