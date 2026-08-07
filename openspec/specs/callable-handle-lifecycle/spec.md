## Purpose

Defines the runtime contract for a callable value that legally crosses the
public boundary (per `callable-public-boundary`): how it is represented once
execution actually produces it, and how it is invoked exactly once.

## Requirements

### Requirement: A callable value crossing the public boundary is observed as an opaque handle
When running a public export whose callable-typed return path resolves, per
`callable-public-boundary`, to one named top-level function, the system
SHALL make that function observable as an opaque handle rather than
rejecting it or exposing the function's internal identity. The handle SHALL
NOT be, or be derived from, the function's internal address or arena
position; it SHALL carry no more information than an opaque identifier that
the runtime alone can resolve.

#### Scenario: Running a qualifying public export produces a handle
- **WHEN** a public export whose return type is callable is executed and its
  return path resolves to a named top-level function with zero ambient
  requirements
- **THEN** the produced observable value is a handle standing for that
  function, not a runtime failure and not the function's internal identity

### Requirement: Invoking a handle runs its target exactly once with the given arguments
The system SHALL provide a runtime operation that, given a live handle and a
list of arguments, invokes the handle's target function with those arguments
and yields the function's result. This operation SHALL consume the handle in
the same operation, whether or not the target function's own execution then
succeeds, so the same handle can never be invoked again. No separate
operation to release a handle SHALL exist; a handle that is never invoked
requires no explicit release.

#### Scenario: A fresh handle can be invoked once
- **WHEN** a handle produced for a named function is invoked with arguments
  matching that function's declared parameters
- **THEN** the target function runs with those arguments and the operation
  yields its result

#### Scenario: A handle is consumed by the invoke operation even when the target fails
- **WHEN** a handle is invoked and the target function's own execution fails
  at runtime
- **THEN** the handle is no longer live once the invoke operation completes,
  the same as if the target had succeeded

### Requirement: A second invoke of the same handle is rejected safely
The system SHALL reject an invoke of a handle that is not currently live —
because it was already consumed by a prior invoke, or because it does not
correspond to any handle the runtime minted — with a runtime diagnostic. This
rejection SHALL use the same runtime-failure representation as every other
runtime failure the interpreter reports; it SHALL NOT be a process abort or
an unrecoverable crash.

#### Scenario: Invoking an already-consumed handle is rejected
- **WHEN** a handle that was already invoked once is invoked a second time
- **THEN** the second invoke is rejected with a runtime diagnostic and the
  target function does not run again

#### Scenario: Invoking an unrecognized handle is rejected
- **WHEN** an invoke is attempted with a value that does not correspond to
  any handle the runtime currently holds live
- **THEN** the invoke is rejected with a runtime diagnostic instead of
  running any function
