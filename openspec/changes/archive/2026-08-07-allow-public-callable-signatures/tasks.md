## 1. Public-export awareness in typecheck

- [x] 1.1 Add a `public_exports: &[module::PublicExport]` parameter to
      `typecheck::check_and_lower`; build a `BTreeSet<&str>` of canonical
      names once at the top for lookup by `sig.name`.
- [x] 1.2 Update both call sites in `src/main.rs` (plain interpret path and
      `build()`) to pass `loaded.public_exports`.
- [x] 1.3 Update `wasm_abi.rs`'s own unit tests / any other direct callers of
      `check_and_lower` (e.g. differential-execution harness, unit tests in
      `typecheck.rs`) to pass an empty or appropriate `public_exports` slice.

## 2. Allow callable return types for public exports

- [x] 2.1 In `lower_ret`, when `sig.name` is in the public-export set, apply
      the same outermost-only/no-optional/no-reference/no-nesting shape rule
      `reject_nested_callable` already applies to parameters, instead of the
      blanket `reject_callable`. Keep `reject_callable` unchanged for
      non-public functions.
- [x] 2.2 Confirm callable-typed parameters need no code change (already
      permitted via `check_signature_shape`'s `callback_ok = true` path for
      all top-level functions) and add a focused test making that explicit
      for a public export.

## 3. Ambient-zero and resolved-target check for public callable exports

- [x] 3.1 Add a new check (in `requirement.rs` or a small dedicated module)
      that, given `checked.hir`, `analysis: &requirement::Analysis`, and the
      `exports: Vec<(String, hir::CallableId)>` list `build()` already
      constructs, walks every callable-typed public export's return paths
      (explicit `return` and final expression) and resolves each via
      `hir::callable_of(body, expr_id, &hir::Bindings::new())`.
- [x] 3.2 Report a source-spanned diagnostic when a return path does not
      resolve to one statically known named function.
- [x] 3.3 Report a source-spanned diagnostic, naming the returned function
      and the required ambient slot(s), when
      `analysis.requirements(hir::BodyId::Callable(target), &hir::Bindings::new())`
      is non-empty for the resolved target.
- [x] 3.4 Wire this check into `build()` in `src/main.rs`, alongside the
      existing `analysis.errors_for_roots(&roots)` call, so both diagnostic
      classes are collected and reported together before
      `ambient_abi::plan_production`.

## 4. Keep the Wasm backend from mis-handling callable public signatures

- [x] 4.1 In `wasm_abi.rs::param_port` and `result_port`, add an explicit
      case for `hir::TypeKind::Callable { .. }` that reports a
      source-spanned "not yet supported by this Wasm build" diagnostic
      (mentioning CAB-030) instead of falling into the generic
      `None if supported(ty) => Port::Rich(...)` branch.
- [x] 4.2 Confirm `wasm::supported()` is left unchanged (it still correctly
      answers the internal-reachability question for `check_support`/
      `check_local`) and add a regression test that a callable-typed public
      signature fails cleanly (diagnostic, not panic) via
      `rhodolite build --target wasm`.

## 5. Tests

- [x] 5.1 Typecheck focused tests: public export with a callable parameter
      is accepted; public export with a callable return type naming a
      zero-requirement function is accepted; non-public function with a
      callable return type is still rejected exactly as before this change;
      nested/optional/referenced callable in a public signature is rejected
      with a source span.
- [x] 5.2 Ambient-zero diagnostic test: a public export returning a named
      function that requires an ambient slot is rejected before Wasm
      generation, with a source span naming the returned function and the
      slot.
- [x] 5.3 Closure-at-public-boundary CLI diagnostic test: a public export
      attempting to return a closure literal is rejected with a source span
      (regression guard for once CLO-020 lands general closure support).
- [x] 5.4 `rhodolite build` CLI diagnostic test: a callable-typed public
      parameter or return value that passes type checking is rejected by
      the Wasm backend with a clear diagnostic, not a panic.
- [x] 5.5 Note in the PR/commit that this task is type-checking only per
      ROADMAP.md's CAB-010 scope: the "interpreter vs Wasm differential
      execution" and "deterministic Wasm bytes" common completion criteria
      do not apply here beyond task 4's regression test that the Wasm build
      fails cleanly rather than crashing.

## 6. Verification

- [x] 6.1 `cargo fmt --check`
- [x] 6.2 `cargo clippy --all-targets -- -D warnings`
- [x] 6.3 `cargo test`
- [x] 6.4 Commit the verified, working state as a snapshot before moving on.
