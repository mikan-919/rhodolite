## Purpose

Defines the maintained verification contract that keeps the checked-HIR
interpreter and generated Core WebAssembly execution semantically aligned for
the supported compiled-v1 Rhodolite surface.

## Requirements

### Requirement: Maintained supported-program differential corpus
The repository SHALL maintain a named corpus of ownership-safe Rhodolite
programs that are supported by the Core Wasm backend. The corpus SHALL include
scalar control flow, owned-data construction and cleanup, explicit shared and
mutable borrows, inherent and trait methods, value and type slots, nested
`with` provisions, provider substitution, generic function and generic trait
method instantiation (both scalar and owned-value cases, with and without
callback-bound ambient requirements), array construction via `push`
(including a capacity-growth boundary and a moved-in non-`Copy` element),
generic array `map` (including an owned element and an ambient-requiring
callback), and the canonical production program. Each corpus member SHALL
identify the entry point and any observable final state required for
comparison.

#### Scenario: Every compiled-v1 feature family is represented
- **WHEN** the maintained differential suite runs
- **THEN** it executes at least one corpus member for each supported feature
  family named above through both execution paths

#### Scenario: Canonical program remains covered
- **WHEN** `examples/canonical.rd` is supported by the production Wasm build
- **THEN** the differential suite runs its production entry through the
  interpreter and generated module and compares their outcomes

#### Scenario: Generic instantiation with an owned value is represented
- **WHEN** the maintained differential suite runs
- **THEN** it executes at least one corpus member that moves a non-`Copy`
  value through a generic function or generic trait method instantiation and
  compares the interpreter's and the generated module's final result

#### Scenario: `push` with a capacity-growth boundary is represented
- **WHEN** the maintained differential suite runs
- **THEN** it executes at least one corpus member that pushes past a
  capacity-doubling boundary (for example, from a length that fills
  capacity 1 into capacity 2) and compares the interpreter's and the
  generated module's resulting array length and contents

#### Scenario: `push` with an owned element is represented
- **WHEN** the maintained differential suite runs
- **THEN** it executes at least one corpus member that moves a non-`Copy`
  value into an array via `push` and compares the interpreter's and the
  generated module's final ownership state for that value

#### Scenario: Generic array `map` with an owned element is represented
- **WHEN** the maintained differential suite runs
- **THEN** it executes at least one corpus member that calls `move
  xs.map(f)` on an array of a non-`Copy` element type and compares the
  interpreter's and the generated module's resulting array and final
  ownership state

#### Scenario: Generic array `map` with an ambient-requiring callback is represented
- **WHEN** the maintained differential suite runs
- **THEN** it executes at least one corpus member that calls `move
  xs.map(f)` where `f` requires an ambient slot provided around the call,
  and compares the interpreter's and the generated module's resulting
  array

### Requirement: Observable execution outcomes are compared
For every corpus member, the suite SHALL execute the same loaded program through
the checked-HIR interpreter and through a freshly generated Core Wasm module in
an independent Wasm engine. It SHALL compare successful return values, process
completion, standard output, declared test results, runtime-error
classification, and fixture-declared final owned or mutable state. A mismatch
SHALL fail the suite with the fixture identity and both normalized outcomes.

#### Scenario: Successful value and state agree
- **WHEN** a corpus member completes successfully after owned mutation or an
  explicit borrow
- **THEN** the interpreter and Wasm engine report equal returned values and
  equal fixture-declared final state

#### Scenario: Runtime failure agrees
- **WHEN** a corpus member reaches a supported runtime failure such as integer
  division by zero
- **THEN** both paths are recorded as the same runtime-error classification and
  the differential suite succeeds for that expected failure case

#### Scenario: Mismatch is actionable
- **WHEN** either path produces a different normalized outcome
- **THEN** the suite fails and reports the corpus member plus the interpreter
  and Wasm observations without accepting either result as authoritative

### Requirement: Differential artifacts are independently executable and deterministic
Every Wasm artifact used by the differential corpus SHALL validate independently,
instantiate without required imports or an automatic start action, and be
executed only through its exported entry or public wrapper. The suite SHALL
retain deterministic generated-module snapshots for representative programs —
including a scalar, an owned-data, an ambient, and a generic array `map`
program — include small generated-program cases, and build every corpus
member twice to assert byte-identical artifacts.

#### Scenario: Generated artifact is run independently
- **WHEN** a corpus member is compiled for Wasm
- **THEN** an independent validator accepts the module and an independent Core
  Wasm engine invokes its required exports

#### Scenario: Repeated build preserves bytes
- **WHEN** the suite builds the same corpus member twice with identical sources
  and options
- **THEN** the two generated module byte sequences are identical

#### Scenario: Representative module shape is pinned
- **WHEN** a representative scalar, owned-data, ambient, or generic array
  `map` corpus member is intentionally changed
- **THEN** its generated-module snapshot exposes the artifact-shape change for
  review
