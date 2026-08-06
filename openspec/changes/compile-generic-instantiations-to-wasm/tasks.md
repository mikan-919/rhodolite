## 1. Owned-value generic differential fixture

- [ ] 1.1 Add a new inline fixture constant to `src/differential.rs`
      (alongside `GENERIC_FILES`/`GENERIC_IMPL_FILES`/`GENERIC_AMBIENT_FILES`)
      that declares one generic function (e.g. `apply<T, U>` or
      `identity<T>`) called with a `move`d non-`Copy` struct or array
      argument through a consuming callback, and register it as a `Fixture`
      in `FIXTURES` (design.md Decision 4).
- [ ] 1.2 Run `cargo test --bin rhodolite differential::` and confirm the
      new fixture passes `maintained_corpus_has_matching_observable_
      outcomes` and `every_fixture_is_byte_deterministic_and_
      independently_executable` unmodified.
- [ ] 1.3 If either test fails, diagnose and fix the gap in
      `src/wasm.rs`/`src/wasm_ambient.rs`/`src/wasm_data.rs` (design.md
      Decision 1's fallback) before proceeding to section 2.

## 2. Wasm-level generic instance snapshot tests

- [ ] 2.1 Add a generic-instantiation instance-signature/ambient-field
      snapshot test to `src/wasm.rs::mod tests`, reusing
      `instance_signature_snapshot`/`plan_of`, modeled on
      `instance署名とambient_localの並びは固定される` and
      `ambient_abi.rs`'s `GENERIC_CALLBACK_SRC`: one generic declaration
      called once under a provider and once with a callback that needs no
      ambient value, asserting stable, distinct compiled signatures and that
      the no-ambient instantiation carries no hidden field.
- [ ] 2.2 Add a test asserting that two distinct callback bindings of the
      same generic declaration produce distinct compiled function targets at
      their call sites (no shared/indirect dispatch point) — reuse the
      compiled module's function-index/call-target inspection already used
      by this module's other instance-signature tests.
- [ ] 2.3 Add a test asserting an unreached generic declaration compiles to
      no function (reachability-only generation), mirroring the shape of
      `ambient_abi.rs`'s `到達しない関数は計画に出ない` but asserted against
      the emitted module.

## 3. Allocator-reuse (no-leak) test for a generic consuming callback

- [ ] 3.1 Add a bounded-loop, capped-page test to `src/wasm.rs::mod tests`
      (modeled on `ループで作った文字列は使い回される`) that calls a generic
      consuming callback with an owned value inside a bounded loop and
      asserts `invoke_capped` succeeds at a tight page count, proving
      repeated generic-instantiation calls reuse freed memory rather than
      leaking it.
- [ ] 3.2 If the test traps unexpectedly (OOM within the page cap),
      diagnose and fix the leak in the affected Wasm backend file(s) before
      proceeding.

## 4. Specs

- [ ] 4.1 Confirm `openspec/changes/compile-generic-instantiations-to-wasm/
      specs/generic-instantiation-wasm/spec.md` and `.../specs/
      differential-execution/spec.md` scenarios each map to a test added in
      sections 1-3; adjust either the spec text or the tests so they match
      exactly (no scenario without a test, no test without a scenario).

## 5. Verification and closeout

- [ ] 5.1 Run the full test suite (`cargo test --bin rhodolite`) and confirm
      all existing and new tests pass.
- [ ] 5.2 Run `cargo fmt --check` and Clippy with warnings as errors; fix
      any findings.
- [ ] 5.3 Commit the stable, passing state as a single snapshot.
- [ ] 5.4 Mark MAP-070 `done` in `ROADMAP.md`.
