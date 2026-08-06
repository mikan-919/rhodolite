## 1. `Map<T>` declaration and `impl<T> Map<T> for [T]` reachability

- [ ] 1.1 Confirm `trait Map<T> { fn map<U>(self, f: fn(T -> U) -> [U]) }`
      declares through existing `GenericOwner::Trait` machinery with no
      code change; add a focused test mirroring `Push<T>`'s "declares its
      method" scenario.
- [ ] 1.2 Add a focused test that `impl<T> Map<T> for [T] { fn map<U>(self,
      f: fn(T -> U) -> [U]) { ... } }` still reports "not a struct" before
      any widening lands (proves the starting point, not a regression).
- [ ] 1.3 Widen `check_generic_impls` (`src/typecheck.rs`) to accept an
      array target whose element is exactly the impl's own type parameter
      (design.md Decision 1), registering it in `Ids.generic_impls` under a
      canonical, element-spelling-independent key while leaving
      `impl_target_name`'s literal diagnostic spelling untouched.
- [ ] 1.4 Confirm a concrete-element array target (`impl<T> Foo<T> for
      [int]`) and a callable-type target still report "not a struct".
- [ ] 1.5 Confirm two `impl<T> Map<T> for [T]` declarations (or one
      `impl<T> Map<T> for [T]` plus one `impl<T> OtherTrait<T> for [T]`
      providing a same-named method) are still diagnosed as a duplicate
      impl / ambiguous call respectively, the same way two struct impls are.

## 2. `xs.map(f)` call-site resolution

- [ ] 2.1 Add the array-receiver arm to `resolve()`'s method-call match
      (`src/typecheck.rs`), after the existing `.clone()`/`.push()`
      interceptions, looking up the sentinel-keyed `Ids.generic_impls`
      entry and delegating to the existing `generic_method`/`generic_call`
      (design.md Decision 2).
- [ ] 2.2 Focused tests: `xs.map(f)` infers `U` from `f`'s return type;
      `.push`/`.clone()` still resolve to their builtins ahead of any
      declared `map`; an array receiver calling an undeclared method still
      reports "no such member" unchanged.

## 3. Instantiation: substituted `self` type and `CallableOwner`

- [ ] 3.1 Extend `instance_owner` (`src/typecheck.rs`) to compute `self`'s
      `KnownType` by substituting the target through the caller's solved
      `bindings` for both struct and array targets, and to return
      `CallableOwner::Free` for an array target (design.md Decision 3).
- [ ] 3.2 Run the existing generic-trait-resolution struct-impl test suite
      unmodified; confirm zero regression (verifies the struct-target
      substitution is a byte-for-byte no-op).
- [ ] 3.3 Focused test: two `map` instantiations over different element
      types each get a correctly-typed `self` and are distinct, separately
      cached instantiations (per `generic-function-instantiation`'s
      existing caching contract).

## 4. `map`'s body and ownership

- [ ] 4.1 Write `impl<T> Map<T> for [T]`'s `map` body using `let mut
      result: [U] = []`, `for x in move self`, `result.push(f(move x))`,
      `move result` (design.md Decision 4).
- [ ] 4.2 Confirm ownership checking accepts the body with zero new code in
      `src/ownership.rs`; if a gap surfaces, record the deviation and the
      minimal fix the same way `add-array-push`'s design recorded its
      Decision-1 deviation.
- [ ] 4.3 Eval focused tests: left-to-right callback order; `Copy` element
      type; non-`Copy` element type (moved into `f`, source array
      unusable after `move xs.map(f)`); empty array (`f` never called,
      empty result).
- [ ] 4.4 Typecheck/ownership focused tests: `move` is required at the
      call site; a borrowed (`&`/`&mut`) `map` call is rejected;
      `xs.clone().map(f)` leaves `xs` usable afterward.

## 5. Ambient requirement propagation

- [ ] 5.1 Change `src/requirement.rs`'s `scan`'s `Call::Method` arm from a
      `Facts.walks`-only push to a full `Facts.calls` requirement edge,
      mirroring `Call::Indirect` (design.md Decision 5).
- [ ] 5.2 Focused requirement-inference tests: a callback's ambient
      requirement passed to `move xs.map(f)` reaches the caller of
      `xs.map(f)`; a callback with no ambient requirement leaves the call
      site clean; a missing-provider diagnostic through `map` names the
      `map` call in its reachability path.
- [ ] 5.3 Run the full existing requirement/ambient test suite; confirm no
      existing struct-method-call scenario regresses (the fix is general,
      not `map`-specific — verify it only adds requirement edges, never
      removes or misattributes one).

## 6. Differential execution

- [ ] 6.1 `map-copy-element` fixture (`src/differential.rs`): `move
      xs.map(f)` over a `Copy` element type.
- [ ] 6.2 `map-owned-element` fixture: `move xs.map(f)` over a non-`Copy`
      element type, comparing final ownership state.
- [ ] 6.3 `map-empty-array` fixture: `move xs.map(f)` over an empty array.
- [ ] 6.4 `map-ambient-callback` fixture: `move xs.map(f)` where `f`
      requires an ambient slot provided around the call.
- [ ] 6.5 Register the fixtures in `FIXTURES`; confirm
      `maintained_corpus_has_matching_observable_outcomes` and
      `every_fixture_is_byte_deterministic_and_independently_executable`
      pass unmodified.

## 7. Specs and close-out

- [ ] 7.1 Confirm every scenario in `generic-map`, the modified
      `generic-trait-resolution` requirement, and the modified
      `differential-execution` scenarios has a corresponding test from
      Sections 1-6.
- [ ] 7.2 `cargo test` passes in full.
- [ ] 7.3 `cargo fmt --check` and `cargo clippy --all-targets -- -D
      warnings` pass.
- [ ] 7.4 Confirm two consecutive builds of each new fixture are
      byte-identical.
- [ ] 7.5 Commit the stable, passing state as a single snapshot.
- [ ] 7.6 Sync the `generic-map` / `generic-trait-resolution` /
      `differential-execution` delta specs to `openspec/specs/`
      (including updating `generic-trait-resolution`'s main-spec `Purpose`
      text, which currently reads "without yet resolving an impl whose
      target is not a declared struct"), archive this change, and mark
      MAP-080 `done` in `ROADMAP.md`.
