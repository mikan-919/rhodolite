## Context

See `proposal.md` for the motivation and
`specs/named-function-values/spec.md` for the user-visible contract. Today,
`TypeKind` has only named and array forms, HIR calls are all direct/method/slot
forms, requirement analysis is keyed only by `BodyId`, and an ambient ABI
`InstanceKey` varies only by selected providers. Those invariants cannot
represent a helper whose requirements depend on the callback passed at one call
site.

## Goals / Non-Goals

**Goals:**

- Add concrete callable types, references to free functions, and calls through
  callable locals or parameters without changing existing direct-call syntax.
- Keep every accepted callback target statically known, then specialize
  requirement analysis and code generation by that target.
- Preserve checked-HIR as the sole boundary for ownership, interpretation,
  requirement analysis, and Wasm generation.

**Non-Goals:**

- Captures, anonymous functions, callable receivers, dynamic dispatch,
  recursive callable data, or a runtime closure ABI.
- Type parameters, generic `map`, array construction APIs, public callable
  parameters/results, and a new CLI command.
- General control-flow analysis for mutable callable variables or callback
  values stored in aggregates.

## Decisions

### 1. Use concrete `fn(...)` types and static free-function references

The type grammar gains `fn(T1, T2 -> R)`. Parameter and result types are the
existing concrete types, including their ownership modes; callable type
comparison is exact. A bare free-function name outside direct-call resolution
lowers to a typed function reference. An immutable `let` may infer this type
from such a reference or use an explicit matching annotation.

Only `CallableOwner::Free` declarations may become values. Method names remain
method/associated/slot syntax and cannot be used as callbacks, which avoids an
implicit receiver or provider-capture convention. Callable values are Copy.

`fn` type syntax is preferred to a bare arrow type because it is visibly a
value-level callable and cannot be confused with the existing function
declaration signature grammar. Treating any path as a callable value was
rejected because it would make associated functions and trait methods acquire
unresolved receiver semantics.

### 2. Keep callable data flow deliberately finite

Typed HIR adds `TypeKind::Callable`, `ExprKind::Function(CallableId)`, and an
`Call::Indirect { callee, args }` form. Type checking permits callable values
only as an immutable local initializer or an argument to a free-function call;
the value can then be copied into a matching callable parameter. The checker
rejects callable return types, mutable callable locals, fields, enum payloads,
and arrays at their declaration or value-use span.

Each successful `Call::Indirect` therefore has a local whose target is supplied
by a finite callback binding context. The context maps callable parameter
`LocalId`s (and immutable aliases when resolving an argument) to a
`CallableId`. A call such as `apply(double, 21)` creates the binding
`apply::f → double`; an indirect `f(value)` is resolved against that binding.

Allowing mutable or aggregate storage was rejected because it needs a separate
flow analysis and would make the callback target a set rather than one static
identity. Closure capture was rejected because the mapping would no longer be
the complete runtime environment.

### 3. Specialize requirements and ABI instances by callback bindings

Introduce a canonical callback-specialization key: the body ID plus its sorted
callable-parameter bindings. Requirement analysis computes requirements and
diagnostic paths for this key, rather than treating a body as one globally fixed
requirement set. The existing body-level listing may show the deterministic
union of its specializations for human-facing inspection, but provider checking
and planning use the exact specialization result.

`ambient_abi::InstanceKey` extends this same key with its current selected
provider vector. When planning a direct free-function call, the planner resolves
callable arguments from the caller's callback context and requests the callee
with those bindings. When planning an indirect call, it resolves the local to
its bound `CallableId` and requests that callback directly. The resulting plan
still records a `PlannedCall` at every executable call expression.

This extends the existing whole-program specialization mechanism instead of
adding source-level effect variables or a function-table ABI. A single
body/callback/provider combination is interned once; recursive request-before-
walk behavior remains unchanged. The key space is finite because callback values
are only references to the program's finite free-function declarations.

### 4. Execute and lower indirect calls as selected direct calls

The checked interpreter represents a callable value as its `CallableId`; binding
and calling it follows the checked callback context and preserves Copy behavior.
The ambient planner gives every indirect call a concrete target instance, so the
Wasm backend emits the same direct Wasm call and forwards the target's hidden
ambient record. It does not emit `funcref`, a table, closure allocation, or a
new exported ABI type.

Keeping an interpreter callable identity is useful for parity tests and
diagnostics. Lowering through a Wasm indirect-call table was rejected because
the accepted source subset already knows the target, while a table would add a
runtime contract that closures will need to revisit.

### 5. Test the feature from parser through differential execution

Add focused parser/type/HIR tests for callable type spelling, value references,
signature mismatches, and rejected storage. Add ownership tests that callable
values copy while ordinary values retain their current rules. Add requirement
and plan tests for two calls to one helper with different callback requirements,
including a missing-provider path. Extend the maintained differential corpus
with scalar and ambient callback fixtures, deterministic Wasm snapshots, and
rebuild-byte checks.

## Risks / Trade-offs

- [Specialization count grows with callback combinations] → The accepted
  callback set is finite and keys are interned; add deterministic unit tests
  for deduplication and use the existing reachability roots.
- [Body-level requirement output can hide per-callback precision] → Keep exact
  specialization requirements authoritative for provider diagnostics and plan
  construction; document any displayed union as an inspection summary.
- [Callback aliases could accidentally bypass the restriction] → Centralize
  callable-origin resolution in type checking and make unsupported storage fail
  before ownership analysis.
- [Direct-call behavior regresses while adding another call form] → Retain the
  existing direct call variants and cover their legacy parser/type/differential
  fixtures in the full suite.

## Migration Plan

1. Land the HIR and checker representation with rejection diagnostics while
   retaining direct-call behavior.
2. Add callback-specialized requirement and ambient planning, then interpreter
   and Wasm lowering behind focused tests.
3. Add differential fixtures, snapshots, documentation, and full verification.
4. Roll back by reverting this change's commits; no source migration, persisted
   data, public ABI, or host integration is introduced.
