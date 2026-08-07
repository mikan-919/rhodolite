## Why

CAB-010 already lets a public export declare and type-check a callable-typed
return value that resolves to one zero-ambient-requirement named function
(`callable-public-boundary`), but the interpreter itself still cannot produce
a runtime value for that case. `eval.rs`'s `public_value` conversion — the
function that turns an internal store value into the observable `Value` a
caller actually sees — still unconditionally rejects `OwnedValue::Function`
with "callable 値は観測できる値になりません" ("callable values cannot become
observable values"), a rule written before CAB-010 legalized exactly this
case. There is also no way for anything outside the interpreter to later
invoke that callable at all: `hir::CallableId` is an internal arena index,
and ADR-0011 forbids handing that out directly. CAB-020 closes this gap on
the runtime (HIR interpreter) side only: it gives a callable value crossing
the public boundary an opaque, single-shot handle representation, and adds
the runtime operation that consumes exactly one handle to run its target and
release it — so CAB-030 can later build the actual Core Wasm invoke export
against interpreter behavior that already matches CAB-000's contract
(CAB-Q2).

## What Changes

- Add a new observable runtime value a running public export can produce for
  a callable-typed return value: an opaque handle standing for one resolved
  named top-level function, instead of the interpreter's current outright
  rejection of any callable value it is asked to make observable.
- Add a runtime "invoke" operation that takes a handle plus arguments, runs
  the target function through the same call machinery as an ordinary
  function call, and consumes the handle in that same operation regardless
  of whether the target's own execution then succeeds — so the handle can
  never be reused, even after a failed invocation (CAB-Q2: single-shot,
  released automatically inside the invoke call, no separate release
  operation is added).
- Reject an invoke of an unknown or already-consumed handle with a runtime
  diagnostic, using the interpreter's existing runtime-failure
  classification (`Flow::Error(Diag)`, the same shape every other runtime
  error already uses) — not a Rust panic and not a new failure category.
- The handle is a small opaque identifier minted by a runtime-owned table; it
  never exposes the underlying `CallableId` (an arena index) or any other
  internal address, matching ADR-0011.
- No CLI surface changes, no `rhodolite build`/Wasm codegen changes, and no
  new "release" export are added by this change. Building the actual Wasm
  invoke export is CAB-030; this change is scoped to the interpreter's
  internal handle mechanism only, as ROADMAP.md's CAB-020 entry specifies.

## Capabilities

### New Capabilities
- `callable-handle-lifecycle`: the runtime contract for how a callable value
  that already legally crosses the public boundary (per
  `callable-public-boundary`) is represented once execution actually
  produces it, and how a single runtime operation consumes a handle exactly
  once to invoke its target.

### Modified Capabilities
(none — `callable-public-boundary`'s type-checking contract and
`named-function-values`'s existing requirements are unchanged by this
change; CAB-020 adds runtime behavior for a case those capabilities already
made legal, without changing what they require.)

## Impact

- `src/eval.rs`: the observable `Value` type gains a variant that carries an
  opaque handle; `Interp<'p>` gains the handle-table state needed for a
  handle minted while running one export to still be valid for a later,
  separate invoke call (this only affects `Interp`'s internal state, not the
  public signatures of its existing `run`/`run_test`/`show` methods);
  `CheckedInterp`'s `public_value`/`checked_public_value` conversion no
  longer rejects `OwnedValue::Function`, and instead mints a handle; a new
  method on `Interp` performs the invoke operation described above.
- Tests are added in `eval.rs`'s existing test module: handle mint + invoke
  + automatic release, and invoking the same handle twice.
- Out of scope, explicitly: `src/wasm_abi.rs`, `src/wasm.rs`, `src/wasm_data.rs`
  (CAB-030's invoke export and CAB-040's ABI v1 metadata), differential
  fixtures pairing interpreter and Wasm behavior (CAB-050 — there is no Wasm
  invoke export yet to compare against), and general marshalling of
  struct/enum/array values through invoke's argument list (only the scalar
  and unit cases the required unit tests exercise are handled; a target
  function whose parameters or result are not yet handled reports a clear
  runtime diagnostic instead of panicking, the same posture CAB-010 already
  established in `wasm_abi.rs` for Wasm codegen it does not yet implement).
