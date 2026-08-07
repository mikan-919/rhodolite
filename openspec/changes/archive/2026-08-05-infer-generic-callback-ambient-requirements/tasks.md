## 1. `requirement.rs` test matrix for generic callback specialization

- [x] 1.1 Test: a generic instantiation's callback requirement reaches the
      caller — `fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }`, a
      function that calls an ambient slot, `apply(that_fn, arg)` inferred to
      require the slot (mirrors the existing non-generic
      `callback特殊化ごとに要求が分かれる` test, but with a generic `apply`).
- [x] 1.2 Test: the same generic declaration's two instantiations (same type
      arguments, different callbacks — one needing a slot, one not) get
      independent `Analysis::requirements(body, bindings)` results; the
      callback-free instantiation's result is empty and does not include the
      other instantiation's slot.
- [x] 1.3 Test: the same generic declaration's two instantiations with
      *different* type arguments (both using a callback that needs a slot)
      each require the slot independently, and are distinct `BodyId`s.
- [x] 1.4 Test: a callback forwarded through a chain of two nested generic
      calls (e.g. `fn relay<T, U>(f: fn(T -> U), x: T -> U) { apply(f, x) }`
      calling generic `apply`) still carries the callback's ambient
      requirement to `relay`'s caller.
- [x] 1.5 Test: a recursive generic helper that forwards its own callback
      parameter on every recursive call (mirroring MAP-040's
      `fn apply<T>(f: fn(T -> T), x: T, n: int -> T) { if n == 0: x else: apply(f, f(x), n - 1) }`)
      still infers the callback's ambient requirement correctly for the one
      instantiation produced.
- [x] 1.6 Test: a missing-provider diagnostic reaching a generic call site
      (no `with` anywhere on the path) reports a reachability path that
      names both the generic helper and the selected callback — same
      assertion shape as the existing
      `間接呼び出しの提供忘れは経路にhelperとcallbackを出す` test, applied to
      a generic `apply<T, U>`.
- [x] 1.7 Test: an inner `with` around a generic call site satisfies the
      callback's requirement and produces no diagnostic (generic analogue of
      `入れ子のwithはcallback越しでも要求を止める`).
- [x] 1.8 Run the full existing `requirement.rs` test suite; confirm no
      existing (non-generic) test's expected output changes.

## 2. `ambient_abi.rs` test matrix for generic instantiations

- [x] 2.1 Test: two instantiations of the same generic declaration with
      different callback bindings, one requiring a provided slot and one
      requiring none, produce two distinct `Instance`s via
      `plan_hir_for_test`, each with its own `RecordLayout`/`layout: None`
      as appropriate, and the callback-free instance's `PlannedCall`s carry
      no provider projection for that slot.
- [x] 2.2 Test: two calls to the same generic instantiation (same type
      arguments, same callback, same providers in scope) share one
      `Instance` via `Plan::intern` — no duplicate instance is created.
- [x] 2.3 Test: a missing provider reaching a generic instantiation's
      requirement produces `PlanError::MissingProvider` with that
      instantiation's `BodyId`, matching the shape of the existing
      non-generic `MissingProvider` test.
- [x] 2.4 Run the full existing `ambient_abi.rs` test suite; confirm no
      existing (non-generic) test's expected output changes.

## 3. Fix the `Analysis` summary-map name collision

- [x] 3.1 In `requirement::analyze_hir`'s summary-construction loop
      (`src/requirement.rs`, the loop building `public`/`order` from
      `program.bodies`), add a local occurrence counter keyed by
      `program.show_body(id)` (consulted/incremented once per body, in the
      existing deterministic iteration order) and use `"{name} #{n}"` (for
      the 2nd and later occurrence of a name) as the key inserted into
      `public` and pushed onto `order`, instead of the bare name.
- [x] 3.2 Test: a program with two reachable instantiations of one generic
      declaration (different callbacks, one needing a slot) produces two
      entries in `Analysis.render()`'s output, each showing that
      instantiation's own requirement set — neither entry is dropped and
      neither shows the other's requirements.
- [x] 3.3 Test: a program with no generic instantiations (or exactly one
      instantiation per generic declaration) produces byte-identical
      `render()`/`Analysis.reqs`/`Analysis.order` output to before this
      task — confirm against the existing `corpusの解析結果は移行前と同じ`
      test and the canonical fixture, unchanged.

## 4. Documentation touch-up

- [x] 4.1 After archiving, edit
      `openspec/specs/generic-function-instantiation/spec.md`'s
      `## Non-Goals` to remove the bullet "Inferring ambient/effect
      requirements through a generic function's callback parameters on a
      per-callback basis (MAP-050)", since this change fulfills it.
- [x] 4.2 Update `ROADMAP.md`'s MAP-050 row status to `done` once complete,
      per the repo's existing roadmap-tracking convention.

## 5. Quality gates

- [x] 5.1 `cargo fmt --check` passes.
- [x] 5.2 `cargo clippy --all-targets -- -D warnings` passes.
- [x] 5.3 Full `cargo test` suite passes.
- [x] 5.4 Commit the verified working state as a snapshot, per repo
      convention.
