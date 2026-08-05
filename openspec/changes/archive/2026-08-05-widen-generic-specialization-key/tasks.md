## 1. Thread callback-parameter bindings through `check_body`

- [x] 1.1 Add `Out.callback_bindings: hir::Bindings`, saved/restored around
      `check_body`'s target switch exactly like `out.body` already is
      (`src/typecheck.rs:2921`).
- [x] 1.2 Add a `callback_key: &[Option<hir::CallableId>]` parameter to
      `check_body`, positional with `params`. In the parameter-allocation
      loop (`src/typecheck.rs:2930-2937`), after allocating each param's
      `LocalId`, insert `(local_id, callable)` into the fresh bindings map
      that becomes `out.callback_bindings` for the duration of this body.
- [x] 1.3 Update the three existing non-generic `check_body` call sites
      (free fn, impl method, test) to pass an all-`None` `callback_key` of
      the right arity (or an empty slice where arity is always zero) —
      zero behavior change for non-generic checking.
- [x] 1.4 Run the full existing test suite and confirm byte-identical HIR
      dumps/diagnostics for every non-generic program.

## 2. Compute a call site's callback key

- [x] 2.1 In `generic_call` (`src/typecheck.rs:2110`), after the existing
      per-argument `synth`/unification loop, compute `callback_key: Vec<Option<hir::CallableId>>`
      by calling `hir::callable_of(&out.body, checked[i].id,
      &hir::resolve_bindings(&out.body, &out.callback_bindings))` for each
      checked argument, in argument order.
- [x] 2.2 Pass `callback_key` into `instantiate()` alongside the existing
      `type_args`.

## 3. Widen the instantiation cache and recursion stack

- [x] 3.1 Change `Out.instances` to
      `Vec<(hir::GenericFnId, Vec<hir::Type>, Vec<Option<hir::CallableId>>, hir::CallableId)>`
      and `Out.building` to
      `Vec<(hir::GenericFnId, Vec<hir::Type>, Vec<Option<hir::CallableId>>)>`.
- [x] 3.2 Update `instantiate()`'s cache lookup and "reserve slot before
      recursing" logic (`src/typecheck.rs:2266-2327`) to compare the full
      triple `(id, type_args, callback_key)`.
- [x] 3.3 Keep the polymorphic-recursion scan comparing only `(id,
      type_args)` (ignore `callback_key`), so a `building`-stack entry that
      shares `id`/`type_args` but differs only in `callback_key` is treated
      as a normal cache miss, not a polymorphic-recursion match.
- [x] 3.4 In `instantiate()`, after allocating the shell and before
      `check_body`, call `check_body` with the resolved `callback_key`
      (Decision 2 of design.md) so the shell's own callable-typed
      parameters seed `out.callback_bindings` for its body.

## 4. Tests

- [x] 4.1 Unit test: two calls with the same type arguments and the same
      callback binding (e.g. `apply(double, 1)` and `apply(double, 2)`)
      still share one instantiation.
- [x] 4.2 Unit test: two calls with the same type arguments and different
      callback bindings (e.g. `apply(double, 1)` and `apply(triple, 1)`)
      produce two distinct instantiations, and the two call sites in the
      caller's body target different `CallableId`s.
- [x] 4.3 Unit test: a generic function that forwards its own callback
      parameter through a recursive call (e.g.
      `fn apply<T>(f: fn(T -> T), x: T, n: int -> T) { if n == 0: x else: apply(f, f(x), n - 1) }`)
      compiles to exactly one instantiation.
- [x] 4.4 Unit test: the existing type-argument polymorphic-recursion
      fixture (`grow`) is still rejected with the same diagnostic wording,
      unaffected by the widened key.
- [x] 4.5 Unit test: the existing generic-impl-method equivalents of 4.1-4.4
      (mirroring MAP-025's `Count`/`Grow` trait fixtures) for
      `generic-trait-resolution`.
- [x] 4.6 Regression: existing non-callback generic tests (`identity`,
      `count`, `free関数とimplメソッドの具体化は同じ型引数でも別`, etc.) keep
      passing unchanged, confirming the widened key degenerates to today's
      key when no call passes a callback argument.
- [x] 4.7 CLI diagnostic test (`tests/cli.rs`): the existing
      `型引数の変わる再帰は実行前に失敗する` /
      `型引数の変わるgeneric_implの再帰は実行前に失敗する` tests still pass
      unchanged.
- [x] 4.8 CLI/snapshot test: a program exercising 4.2's shape end-to-end
      through the CLI, observable via an existing dump/diagnostic path (no
      new CLI flag), confirming distinct instantiations are produced and
      reachable/unreachable instantiations match expectations (no
      instantiation for a callback binding no call site uses).

## 5. Documentation touch-up

- [x] 5.1 After archiving, update `openspec/specs/generic-function-instantiation/spec.md`'s
      `## Non-Goals` to remove the callback-identity bullet (fulfilled by
      this change) and add a short note on why callback-binding divergence
      is not a polymorphic-recursion trigger (mirrors design.md Decision 4).
- [x] 5.2 Update `openspec/specs/generic-trait-resolution/spec.md`'s
      `## Non-Goals` the same way.
- [x] 5.3 Update `ROADMAP.md`'s MAP-040 row status once complete, per the
      repo's existing roadmap-tracking convention.

## 6. Quality gates

- [x] 6.1 `cargo fmt --check` passes.
- [x] 6.2 `cargo clippy --all-targets -- -D warnings` passes.
- [x] 6.3 Full `cargo test` suite passes.
- [x] 6.4 Commit the verified working state as a snapshot, per repo
      convention.
