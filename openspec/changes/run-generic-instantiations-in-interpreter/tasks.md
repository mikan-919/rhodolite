## 1. `src/eval.rs` test matrix for generic execution

- [ ] 1.1 Test: `identity<T>(x: T -> T) { x }` called with two different
      concrete type arguments (e.g. `int` and `bool`) in the same program,
      each result combined into the entry point's return value — confirms
      the same generic body produces independent, correct results per
      instantiation through `Interp::new_checked(...).run(...)`.
- [ ] 1.2 Test: `apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }` called
      twice with the same type arguments but two different named-function
      callback bindings — confirms each call runs its own bound callback,
      not the other call's.
- [ ] 1.3 Test: a generic trait method instantiation (`trait Box<T> { fn
      wrap<U>(&self, value: T, f: fn(T -> U) -> U) }`, `impl<T> Box<T> for
      Container { ... }`) called through an ordinary method-call expression
      at two different type arguments — confirms `Call::Method` dispatch to
      a generic impl's instantiation runs to completion like a non-generic
      method.
- [ ] 1.4 Test: `apply<T, U>(f: fn(T -> U), x: T -> U) { f(move x) }` called
      with a non-`Copy` struct argument — confirms the interpreter accepts
      and runs the call, the callback observes the moved value exactly
      once, and the run completes with no store leak (the existing
      `dispose()` debug-mode leak assertion already fires on regression;
      the test's job is to exercise this generic path).
- [ ] 1.5 Test: the same generic declaration called twice in one program
      with two different type arguments, each passing its own non-`Copy`
      value into a `move`d consuming callback — confirms two distinct
      instantiations each move and drop their own value independently, with
      no leak across either.
- [ ] 1.6 Test: a generic function's callback reads an ambient value
      (`effect clock: Clock`), invoked inside a `with` block providing it —
      confirms the interpreter resolves the ambient requirement through the
      generic call and the callback observes the provided value.
- [ ] 1.7 Test: the same generic declaration as 1.6, in the same program,
      also called bound to a callback that reads no ambient value, outside
      any `with` block — confirms that instantiation runs to completion
      without requiring any ambient binding in scope.
- [ ] 1.8 Run the full existing `src/eval.rs` test suite; confirm no
      existing (non-generic) test's expected output changes.

## 2. Fix any discovered gap

- [ ] 2.1 If any test in section 1 fails against the current interpreter,
      fix the gap in `src/eval.rs` (per design.md Decision 1, no gap is
      expected; if one is found, it is fixed here, not deferred) and
      re-run the full suite.

## 3. Documentation

- [ ] 3.1 Confirm `specs/generic-instantiation-execution/spec.md`'s
      scenarios each map to one of the tests added in section 1.
- [ ] 3.2 Update `ROADMAP.md`'s MAP-060 row status to `done` once complete,
      per the repo's existing roadmap-tracking convention.

## 4. Quality gates

- [ ] 4.1 `cargo fmt --check` passes.
- [ ] 4.2 `cargo clippy --all-targets -- -D warnings` passes.
- [ ] 4.3 Full `cargo test` suite passes.
- [ ] 4.4 Commit the verified working state as a snapshot, per repo
      convention.
