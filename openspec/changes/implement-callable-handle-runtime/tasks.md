## 1. Opaque handle type and table

- [x] 1.1 Add an opaque handle type (name is the implementer's choice, e.g.
      `CallableHandle`) that carries only a runtime-minted identifier — not
      `hir::CallableId` and not any other internal address (design.md
      Decision 2).
- [x] 1.2 Add a handle table (name/type is the implementer's choice, e.g. a
      `HashMap`/`BTreeMap` from the minted identifier to `hir::CallableId`)
      with a `mint(CallableId) -> Handle` operation and a `take(Handle) ->
      Option<CallableId>` operation that removes the entry if present
      (design.md Decisions 1-3). No public "release" operation is added.
- [x] 1.3 Add the table as a field on `Interp<'p>` behind interior
      mutability (e.g. `RefCell`), so `run`/`run_test`/`show`'s existing
      `&self` signatures do not change (design.md Decision 1).

## 2. Minting a handle when a callable value becomes observable

- [x] 2.1 Add a variant to `eval::Value` that carries the opaque handle type
      from 1.1.
- [x] 2.2 In `CheckedInterp::public_value`, replace the current
      unconditional rejection of `OwnedValue::Function(_)`
      (`"callable 値は観測できる値になりません"`) with minting a handle via
      the table from 1.2 and returning the new `Value` variant (design.md
      Decision 5). Give `CheckedInterp` whatever access to the table it
      needs to do this (e.g. a reference passed into `CheckedInterp::new`
      alongside `program`/`plan`).
- [x] 2.3 Update `Interp::show` to render the new `Value` variant (any
      readable form is fine; this is CLI/debug display only, not a wire
      format).

## 3. Invoke operation

- [x] 3.1 Add an invoke method on `Interp` (name is the implementer's
      choice) taking a handle and a list of arguments (`Vec<eval::Value>` is
      the natural choice, matching what `public_value` already produces).
- [x] 3.2 Look up the handle via the table's `take` operation from 1.2
      first, before running anything. If it returns `None` (already
      consumed or never minted), return `Err(Flow::Error(Diag::msg(...)))`
      — do not run any function (design.md Decisions 3-4).
- [x] 3.3 If `take` returns the target `CallableId`, convert the scalar/unit
      argument cases (`Value::Int`, `Value::Str`, `Value::Bool`,
      `Value::Unit`, `Value::Nil`) into the internal representation and run
      the target through the same call machinery `run`/`run_test` already
      use (fresh per-call `Store`, empty `Bindings`/ambient — matching how
      production roots are already planned). For an argument or a result
      that would require struct/enum/array marshalling, return a clear
      `Flow::Error` diagnostic instead of panicking (design.md Decision 6;
      out of scope per proposal.md Impact).
- [x] 3.4 Convert the target's result back to `eval::Value` via
      `public_value` (reusing 2.2's handle-minting behavior automatically if
      the result is itself callable-typed).

## 4. Tests

- [x] 4.1 In `eval.rs`'s test module, add a helper that type-checks and
      lowers a source string with a non-empty `public_exports` list (a
      `module::PublicExport { name, canonical, span }` naming the exporting
      function), matching how `typecheck::check_and_lower` is already
      called with `&[]` in the existing `run`/`checked_run` test helpers —
      the new helper needs a real list so CAB-010's callable-return
      relaxation applies.
- [x] 4.2 Unit test: running a public export that returns a callable value
      naming a zero-ambient-requirement function produces the new `Value`
      handle variant (not a runtime failure).
- [x] 4.3 Unit test: invoking that handle with matching scalar arguments
      runs the target function and yields the expected result.
- [x] 4.4 Unit test: invoking the same handle a second time is rejected
      with a `Flow::Error` diagnostic, and does not run the target function
      again (assert on some observable side effect count, or on the
      function's result not appearing twice).
- [x] 4.5 Unit test: invoking a handle-shaped value that was never minted by
      the table (e.g. constructed independently, or the same identifier
      after the table has been dropped/replaced) is rejected the same way,
      not a panic.

## 5. Verification

- [x] 5.1 `cargo fmt --check`
- [x] 5.2 `cargo clippy --all-targets -- -D warnings`
- [x] 5.3 `cargo test`
- [x] 5.4 Confirm no files under `src/wasm*.rs` changed (this change is
      scoped to the interpreter's internal handle mechanism only; CAB-030
      owns the Wasm invoke export per ROADMAP.md).
- [x] 5.5 Commit the verified, working state as a snapshot before moving on.
