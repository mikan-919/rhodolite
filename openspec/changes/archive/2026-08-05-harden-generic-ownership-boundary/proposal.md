## Why

MAP-020 (`archive/2026-08-05-add-generic-function-instantiation`) and MAP-025
(`archive/2026-08-05-add-generic-trait-resolution`) both monomorphize a
generic call site into an ordinary, fully concrete `hir::Callable` and rely
on ownership checking (`src/ownership.rs`) needing zero new code to process
it. That reliance is structurally sound — `hir::Type`/`TypeKind` has no
variant that can express an unresolved type parameter, so a value shaped
like one cannot be constructed once `check_and_lower` returns `Ok` — but
neither task's own scope included proving it: MAP-020 added exactly one
ownership-focused test (a consuming callback's non-`Copy` argument), and
MAP-025's spec explicitly defers "any new ownership-specific rule or
dedicated ownership test matrix" to MAP-030
(`openspec/specs/generic-trait-resolution/spec.md`, Non-Goals). Today there
is no test exercising Copy classification across distinct instantiations of
the same generic declaration, no test exercising a generic impl method's
consuming `self`/non-`Copy` argument, and no regression test that would
catch a future change accidentally letting a type-parameter placeholder
reach ownership checking. MAP-030 closes that gap before MAP-040/MAP-060/
MAP-070 build further on top of the same boundary.

## What Changes

- Add a focused ownership-behavior test matrix exercising generic free
  function and generic impl method instantiations: `Copy` type arguments,
  non-`Copy` type arguments, move-after-use rejection, deterministic drop
  planning, and a consuming callback (`apply<T, U>`) moving a non-`Copy`
  argument exactly once — for both free functions (MAP-020) and impl
  methods (MAP-025).
- Add a regression test that treats "no type variable reaches ownership
  checking" as an observable, checked property rather than an assumption:
  compiling a program with several distinct instantiations of the same
  generic declaration and confirming the checked HIR/ownership dump never
  contains a rigid-check placeholder spelling (`#T<index>`, the only textual
  form a leaked type parameter could take) and that every instantiation's
  `Copy`/owned classification matches its own concrete type arguments, not
  another instantiation's.
- No production code change is expected: MAP-010's `GenericType`/`Type`
  split already makes the boundary structural. If any of the new tests
  fails, the discovered gap is fixed in `src/ownership.rs`/`src/typecheck.rs`
  as part of this change rather than deferred.
- Document the invariant explicitly in `openspec/specs/generic-ownership-boundary/spec.md`
  so later tasks (MAP-040, MAP-060, MAP-070) can cite a fixed contract
  instead of re-deriving it from MAP-020/MAP-025's design notes.

## Capabilities

### New Capabilities
- `generic-ownership-boundary`: the contract that ownership checking only
  ever observes fully concrete, type-variable-free HIR produced by generic
  instantiation, and that Copy/owned classification, move planning, and
  drop planning for an instantiation follow its own concrete type
  arguments.

### Modified Capabilities
(none — `generic-function-instantiation` and `generic-trait-resolution`
already state instantiations are ordinary concrete HIR; this change adds
the dedicated verification they both deferred, without changing either
spec's requirements)

## Impact

- `src/ownership.rs`: new `#[cfg(test)]` cases only, unless a gap is found.
- `src/typecheck.rs`: no expected change, unless a gap is found.
- `openspec/specs/generic-ownership-boundary/spec.md`: new spec file.
- No public API, data representation, or runtime contract changes.
