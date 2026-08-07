## Why

MAP-010 gave `trait`/`impl` type parameter syntax and MAP-020 made a generic
*free function* callable, but a generic `trait` method's contract is never
checked against its `impl`s and a method call can never resolve to a generic
`impl` — every generic trait method and generic impl method parses, is
recorded as an unresolved `GenericDecl`, and then sits inert forever, exactly
as MAP-010 left it. MAP-025 is the next step on the roadmap to generic `map`:
`Map<T>`'s eventual `impl<T> Map<T> for [T]` (MAP-080) needs a compiler that
can check a generic `impl` against its trait's contract and pick the right
`impl` and method type arguments from a concrete receiver and arguments,
before ownership (MAP-030), the whole-program specialization key (MAP-040),
or the real `map` trait itself (MAP-080) can be built on top of it.

## What Changes

- Contract-check every generic `trait` method against every generic `impl`
  method that claims to provide it: substitute the `impl`'s trait-reference
  type arguments through the trait's declared signature, match the trait
  method's own type parameters against the `impl` method's own type
  parameters by declaration position, and compare receiver, parameter, and
  return shapes — the same shapes today's non-generic `check_impl` compares,
  generalized to signatures that still contain type parameters.
- Validate a generic `impl`'s trait reference itself: the referenced name
  must be a declared trait, and its type-argument count must match that
  trait's own declared arity — both currently unchecked for a generic `impl`
  (MAP-010 deferred this validation here explicitly).
- Reject, with a source-positioned diagnostic: an `impl` missing a
  trait-declared method, an `impl` implementing a method twice, two `impl`
  blocks providing the same trait for the same type, and a provided method
  whose receiver, parameters, or return type don't match the trait's
  contract once type parameters are substituted and corresponded.
- Type-check a generic `impl` method's body once, rigidly, exactly like
  MAP-020 already does for a generic free function's body — so a
  never-called generic `impl` method with a genuine type error is still
  reported, and its own type parameters (and any of its enclosing `impl`'s)
  are opaque, self-distinct types during that check.
- At a method-call site (`recv.method(args)`) whose receiver's concrete type
  finds no match through today's existing (non-generic) method lookup,
  search declared generic `impl`s of that type for a method with that name;
  infer every remaining type argument from the call's arguments the same way
  MAP-020 infers a generic free function's type arguments; diagnose an
  unresolved or conflicting type argument, and diagnose ambiguity when more
  than one generic `impl` could provide the method for that receiver type.
- A resolved call is monomorphized into an ordinary, concrete `hir::Callable`
  the same way MAP-020 monomorphizes a generic free function call, cached by
  declaration and type arguments so repeat calls share one instance. This
  task scopes the shape of `impl` this can apply to (see Design's Decision
  1); the real `Map<T>` trait and its array `impl` are built on top of this
  in MAP-080, once MAP-075 settles how a body constructs an array result.
- Existing non-generic `trait`/`impl` method resolution, and its existing
  diagnostics, are unchanged: the new search only runs after today's lookup
  finds nothing.

## Capabilities

### New Capabilities
- `generic-trait-resolution`: contract-checking a generic `impl` against its
  trait's declared signature (substitution plus positional type-parameter
  correspondence), rigid whole-body checking of a generic `impl` method,
  method-call resolution from a concrete receiver and arguments into a
  unique generic `impl` and method type arguments, the missing/duplicate/
  ambiguous/mismatched-signature diagnostics, and monomorphization of a
  resolved call into a cached, concrete, executable `hir::Callable`.

### Modified Capabilities
- `generic-type-parameters`: MAP-010's requirement that "a generic trait
  method or generic impl method is never registered as a callable, method,
  or trait implementation reachable by [ownership/requirement
  analysis/interpreter/Wasm]" now has the same exception MAP-020 already
  carved out for a called generic free function: a method call that resolves
  to a generic `impl` produces a separate, fully concrete instantiation that
  those passes do process. An uncalled generic `trait`/`impl`, and a `trait`
  declaration itself, still never reach execution.

## Impact

- `src/hir.rs`: add a `trait`'s own declared type parameters to
  `TraitDecl` (needed to split a trait method's combined type-parameter list
  into "trait-level" vs "method's own" for substitution); add a
  `GenericType`-to-`GenericType` substitution helper (mirrors the existing
  `GenericType`-to-`hir::Type` `substitute`, but keeps unresolved leaves as
  type parameters instead of requiring a total binding).
- `src/typecheck.rs`: add contract-checking for generic `impl`s (trait
  reference validation, substitution, positional correspondence, shape
  comparison, missing/duplicate diagnostics); add rigid whole-body checking
  for generic `impl` methods (reuses MAP-020's `check_rigid`/`check_body`
  machinery); add method-call resolution and monomorphization for a generic
  `impl` (reuses MAP-020's unification, bindings, and instantiation-cache
  machinery, generalized to a receiver-typed search over candidate `impl`s
  instead of a name lookup).
- `src/ownership.rs`, `src/eval.rs`, `src/wasm.rs`, `src/requirement.rs`,
  `src/ambient_abi.rs`: unchanged — a resolved call's instantiation is an
  ordinary `hir::Callable` these passes already process generically, the
  same "zero new code downstream" property MAP-020 established for generic
  free functions.
- No new CLI command, public ABI, or external dependency. Existing
  differential and Wasm byte-determinism checks extend to cover a program
  that calls a resolved generic `impl` method.
