## Why

MAP-Q3/MAP-Q3A/MAP-Q4 (ROADMAP.md Decisions) already fixed `map`'s shape: a
`Map<T>` trait with a consuming `map<U>(self, f: fn(T -> U) -> [U])` method,
implemented for `[T]` in ordinary Rhodolite source using MAP-075's `push`.
But nothing today can reach that implementation: `check_generic_impls`
(`src/typecheck.rs:1698-1725`) rejects any generic `impl` whose target is not
a declared struct with a "`{target}` は struct ではありません" diagnostic —
a limitation `add-generic-trait-resolution`'s design explicitly scoped itself
to and deferred outward ("Widening `hir::CallableOwner`/`hir::TraitImplDecl`
to a non-struct target is left to whichever task first needs it
(MAP-075/MAP-080, which build the array `impl` `map` itself needs)"). Method
call resolution has the same gap on the receiver side: `KnownType::name()`
returns `None` for an array (`src/typecheck.rs:183-188`), so `resolve()`'s
generic-impl lookup arm (`Some(type_name) if ... generic_impls.get(...)`,
`src/typecheck.rs:3987-4004`) never even attempts to match an array
receiver. Separately, `src/requirement.rs`'s `scan` deliberately does not
turn a value-receiver method call into a requirement edge — a documented,
pre-generics conservative approximation (`src/requirement.rs:285-291`,
`hir::Call::Method`'s arm only pushes to `Facts.walks`, never
`Facts.calls`) — which `infer-generic-callback-ambient-requirements`'s
design explicitly left alone and assigned to this task: "MAP-080's own
completion criteria explicitly own 'callback の ambient 要求が `map` と
trait dispatch を経由して正確に伝播する'". Without closing these three gaps,
`Map<T>`/`impl<T> Map<T> for [T]` cannot be declared reachably, `xs.map(f)`
cannot resolve, and even a resolved call would silently under-report `f`'s
ambient requirement to its caller.

## What Changes

- Add `trait Map<T> { fn map<U>(self, f: fn(T -> U) -> [U]) }` to the
  standard trait-declaration surface (an ordinary `GenericOwner::Trait`;
  MAP-025's machinery already declares generic traits with no change needed).
- Widen generic-impl contract-checking (`check_generic_impls`) and the
  generic-impl index (`Ids.generic_impls`) to accept a target of `[T]`
  (an array whose element is the impl's own type parameter), alongside the
  existing struct target, using a canonical, element-type-independent key so
  `impl<T> Map<T> for [T]`'s registration does not depend on which letter its
  type parameter happens to spell. Every other non-struct target (a callable
  type, a builtin, a concrete-element array, a blanket `impl<T> Trait<T> for
  T`) stays rejected exactly as it is today.
- Widen method-call resolution (`resolve()`'s array-receiver branch,
  `src/typecheck.rs`) to search declared generic array impls for a method
  when the receiver is an array — after the existing `.clone()`/`.push()`
  builtin interceptions, so those two keep taking priority — reusing the
  existing single-candidate-or-ambiguous / type-argument-inference machinery
  (`generic_method`/`generic_call`) untouched.
- Extend generic-impl instantiation (`instance_owner`) to compute `self`'s
  concrete type by substituting the impl's solved type arguments through its
  array target, instead of only handling a fixed struct name; the
  instantiated body's `CallableOwner` is `Free` (mirroring a generic free
  function — there is no `StructId` for an array target to reference for
  naming or slot dispatch, and `map` is never called through `with`/ambient
  slots).
- Implement `impl<T> Map<T> for [T]`'s `map` body once, in ordinary,
  type-checked Rhodolite source (not a compiler-synthesized body like
  `Push<T>::push`), using a `let mut result: [U] = []`, `for x in move self`,
  and `result.push(f(move x))` — MAP-075's `push` plus the array-literal and
  `for`-loop rules `array-type-checking` already fixes.
- Fix `src/requirement.rs`'s `scan` so `hir::Call::Method` contributes a real
  requirement edge (mirroring `hir::Call::Indirect`) instead of only a
  reachability-only walk, so a callback's ambient requirement propagates
  from `f` through `map`'s specialization and through the `xs.map(f)`
  call site to its caller, matching what already happens for a directly- or
  indirectly-called generic helper.
- Add differential (interpreter vs. independent Wasm engine) coverage for
  `map`: a `Copy` element type, a non-`Copy`/owned element type, an empty
  array, and a callback with an ambient requirement, confirming `move
  xs.map(f)` consumes `xs` while `xs.clone().map(f)` leaves the original
  array intact (no borrowed `map` is added).

## Capabilities

### New Capabilities
- `generic-map`: the `Map<T>` trait, its ordinary-Rhodolite-source `[T]`
  impl body, `map`'s consuming call-site contract (`move xs.map(f)` /
  `xs.clone().map(f)`), left-to-right per-element callback application, and
  interpreter/Wasm parity for `Copy`, non-`Copy`, empty, and ambient-callback
  arrays.

### Modified Capabilities
- `generic-trait-resolution`: the "A generic impl's target must name a
  declared struct" requirement widens to also accept `[T]` (an array whose
  element is the impl's own type parameter), replacing its current rejection
  scenario for that specific shape.
- `differential-execution`: the maintained-corpus requirement's named
  feature-family list is extended to include generic `map` over an array,
  including an owned-element and an ambient-callback case.

## Impact

- `src/typecheck.rs`: generic-impl target-kind widening in
  `check_generic_impls`/`Ids.generic_impls` (array case, canonical key),
  the array-receiver arm in `resolve()`'s method-call resolution, and
  `instance_owner`'s self-type substitution for a non-struct target.
- `src/requirement.rs`: `scan`'s `hir::Call::Method` arm changes from a
  `Facts.walks`-only push to a full `Facts.calls` requirement edge.
- `src/hir.rs`: none expected — `GenericOwner::Impl.target` is already a
  `GenericType` that can spell `[T]`, and `hir::Call::Method` already
  carries the receiver and resolved concrete `CallableId` a struct method
  call carries.
- `src/ownership.rs`, `src/eval.rs`, `src/wasm.rs`: none expected — `map`'s
  instantiated body is an ordinary generic-instantiated `hir::Callable`
  dispatched through the existing `Call::Method` shape, which MAP-020/MAP-060
  /MAP-070 already execute and compile with zero struct-specific assumptions
  once it type-checks and is reachable; this is verified, not assumed,
  during implementation.
- `src/differential.rs`: new maintained-corpus fixture(s) for `map`,
  including an owned element and an ambient-requiring callback.
- `docs/grammar.md`: already sketches `trait Map<T>`/`impl<T> Map<T> for
  [T]`'s declaration shape (lines 527-533) as an illustrative, not-yet-live
  example; no grammar-doc change expected, only confirmation it now executes.
- `openspec/specs/generic-map/spec.md` (new), `openspec/specs/generic-trait-
  resolution/spec.md` (modified), `openspec/specs/differential-execution
  /spec.md` (modified).
- `ROADMAP.md`: MAP-080 status `in-progress` → `done`.
- No public Rhodolite ABI or CLI surface change; no borrowed `map`, explicit
  type-argument syntax, or generic struct/enum added.
