## MODIFIED Requirements

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
