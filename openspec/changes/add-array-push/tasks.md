## 1. `Push<T>` declaration and compiler-synthesized `[T]` impl

- [ ] 1.1 Add `trait Push<T> { fn push(&mut self, x: T) }` support to the
      trait declaration surface (parser, if needed, and `Decls`/HIR trait
      registration), matching how other generic traits are declared
      (design.md Decision 1).
- [ ] 1.2 Register `impl<T> Push<T> for [T]` during declaration collection
      (near `collect()`, `src/typecheck.rs:692`) with a constructed
      signature and a new builtin-body marker (e.g. a
      `hir::CallableBody::Builtin` variant) instead of a parsed body
      (design.md Decision 1).
- [ ] 1.3 Add one new match arm at each pass that switches on a callable's
      body kind (generic specialization keying / MAP-040, ambient
      requirement inference / MAP-050, ownership planning / MAP-030,
      ambient ABI planning) so the builtin `push` body is treated as: no
      requirements, receiver mode `&mut`, argument `x` moved once, no body
      to walk further.
- [ ] 1.4 Add typecheck focus tests: `Push<T>` trait declares correctly;
      `xs.push(y)` resolves to the compiler-synthesized impl for at least
      two distinct element types; a call requires no ambient requirement
      (requirement-inference focus test).

## 2. Receiver-modifier exception and ownership

- [ ] 2.1 Add the `conform_receiver` (`src/typecheck.rs:4218`) exception:
      when the resolved callee is specifically the compiler-synthesized
      `Push<T>::push` impl (checked by resolved impl identity, not by
      method name), auto-insert the exclusive borrow instead of requiring
      a call-site `&mut` (design.md Decision 2).
- [ ] 2.2 Add ownership planning for `push`'s exclusive-borrow receiver
      effect and its move-once second argument, matching how any other
      `&mut self` call plans its receiver borrow and owned argument.
- [ ] 2.3 Add focus tests: `xs.push(y)` typechecks with no `&mut` written;
      calling `push` while another borrow of `xs` is live is rejected the
      same way any other exclusive-borrow conflict is; passing a bound
      non-`Copy` local as `y` moves it (later use rejected); a
      user-declared, unrelated method named `push` on a non-array type
      still requires its normal receiver modifier (negative test proving
      the exception is scoped to the resolved `Push<T>` impl, not the
      name).

## 3. Interpreter execution

- [ ] 3.1 Implement `push` in `src/eval.rs` as a direct `Vec::push` on the
      already-`Vec`-backed `StoredValue::Array`, evaluating and moving in
      `y` under the existing owned-argument path (design.md Decision 4).
- [ ] 3.2 Add eval-level tests: repeated pushes produce the expected
      content/length/order; pushing a non-`Copy` struct/array moves it in
      and the source binding is unusable afterward.

## 4. Core Wasm codegen

- [ ] 4.1 Add a per-array-layout `push` generated function (alongside
      `array_clone`/`array_drop` in `src/wasm_data.rs`, wired the same way
      through the reachable-layout loop, `src/wasm_data.rs:440-491`):
      read `len`/`capacity`; if `len < capacity`, write the new element at
      the next slot and increment `len`; else compute new capacity (1 if
      0, else doubled), `alloc` a new buffer, `MemoryCopy` the live
      elements, `free` the old buffer (skip `free` when `data` is the
      reserved empty-array sentinel address), store new `data`/`capacity`,
      then append and increment `len` (design.md Decision 3).
- [ ] 4.2 Wire `push` call-site codegen in `src/wasm.rs` to call the
      generated per-layout `push` function, consistent with how other
      resolved builtin/trait method calls already lower to a direct call
      by function index.
- [ ] 4.3 Add `src/wasm.rs`-local snapshot/unit tests pinning the 0→1→2→4
      capacity-growth sequence (asserting `len`/`capacity` at each step)
      and confirming prior elements survive a grow unchanged.
- [ ] 4.4 Add a capped-page, bounded-loop test (modeled on the existing
      `ループで作った文字列は使い回される` pattern) that pushes repeatedly
      inside a bounded loop and asserts `invoke_capped` succeeds at a
      tight page count, proving grown/freed buffers are reused rather than
      leaked.
- [ ] 4.5 Confirm an allocator failure during a `push`-triggered grow
      traps via the existing `unreachable`/`memory.grow`-failure path
      (`src/wasm_runtime.rs`'s `extend`) with no new push-specific error
      value; add a regression test if one does not already cover this via
      the capped-page technique above.

## 5. Differential coverage

- [ ] 5.1 Add an inline fixture to `src/differential.rs` (alongside the
      existing `GENERIC_*` constants) that pushes past at least one
      capacity-doubling boundary and register it in `FIXTURES` (design.md
      Decision 5).
- [ ] 5.2 Add a second inline fixture that moves a non-`Copy` value (e.g.
      a small struct) into an array via `push` and register it in
      `FIXTURES`.
- [ ] 5.3 Run `cargo test --bin rhodolite differential::` and confirm both
      new fixtures pass `maintained_corpus_has_matching_observable_
      outcomes` and `every_fixture_is_byte_deterministic_and_
      independently_executable` unmodified; diagnose and fix any gap in
      `src/wasm.rs`/`src/wasm_data.rs`/`src/wasm_runtime.rs` before
      proceeding.

## 6. Specs and closeout

- [ ] 6.1 Confirm `openspec/changes/add-array-push/specs/array-push/
      spec.md`, `.../specs/method-call-type-checking/spec.md`, and `.../
      specs/differential-execution/spec.md` scenarios each map to a test
      added in sections 1-5; adjust either the spec text or the tests so
      they match exactly.
- [ ] 6.2 Run the full test suite (`cargo test --bin rhodolite`) and
      confirm all existing and new tests pass.
- [ ] 6.3 Run `cargo fmt --check` and Clippy with warnings as errors; fix
      any findings.
- [ ] 6.4 Confirm two consecutive builds of each new differential fixture
      are byte-identical (ROADMAP's MAP-010–MAP-100 common completion
      condition).
- [ ] 6.5 Commit the stable, passing state as a single snapshot.
- [ ] 6.6 Sync the `array-push`, `method-call-type-checking`, and
      `differential-execution` delta specs into `openspec/specs/` and
      mark MAP-075 `done` in `ROADMAP.md`.
