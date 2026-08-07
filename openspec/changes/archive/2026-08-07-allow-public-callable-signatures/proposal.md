## Why

CAB-000 fixed the observable contract for exposing callable values across the
public Wasm ABI (CAB-Q1〜CAB-Q5 in `ROADMAP.md`): only named top-level
function values may cross the boundary, only when every reachable ambient
requirement is already zero, because the host has no way to satisfy a
`with`-style provider. Today the checker still rejects any callable type
written as a public function's return type outright (`reject_callable` in
`src/typecheck.rs`), and it never checks whether a callable type that already
type-checks (as a public function's parameter) actually satisfies the
zero-ambient-requirement rule. Nothing currently enforces CAB-Q3's
named-function-only restriction or CAB-Q4's ambient-zero restriction at the
public boundary, and the Wasm backend (`src/wasm_abi.rs`) would currently
mis-handle a callable-typed public signature as ordinary serializable data
instead of rejecting it. CAB-010 closes this gap at the type-checking layer
so later tasks (CAB-020/030/040) can build the actual handle runtime on top
of a signature contract that is already verified.

## What Changes

- Allow a callable type `fn(P1, P2 -> R)` to be written as the return type of
  a function selected as a public export (`pub use`), matching the existing
  allowance for callable-typed parameters. Non-public functions keep
  rejecting callable return types exactly as today.
- Add a new check that walks every public export whose signature contains a
  callable type and rejects it, with a source span, when the callable value
  it returns is not a direct reference to one statically known named
  top-level function, or when that function's own inferred ambient
  requirements are not empty (CAB-Q3, CAB-Q4).
- Confirm and cover with a test that a closure literal can never reach a
  public signature: closure literals are already rejected everywhere in the
  language (CLO-020 has not landed), so this is presently a corollary of
  existing behavior, not new logic — the new test exists to catch a
  regression once CLO-020 lands closures generally.
- Make the Wasm backend (`src/wasm_abi.rs`) reject a callable-typed public
  parameter or return value with a clear, source-spanned "not yet supported"
  diagnostic instead of silently mis-treating it as ordinary owned data
  (which would panic later while building ABI v1 metadata). This keeps
  `rhodolite build` well-behaved for the newly-legal signatures until CAB-030
  implements the actual per-signature invoke export.

## Capabilities

### New Capabilities
- `callable-public-boundary`: the type-checking contract for exposing named
  top-level function values across the public Wasm ABI boundary — which
  callable types may appear in a public function's parameters and return
  type, and the ambient-zero requirement enforced there with source-spanned
  diagnostics.

### Modified Capabilities
- `named-function-values`: the existing "callable value placed in ... return
  value" rejection is narrowed to keep rejecting non-public functions while
  allowing a public export's return type to carry a callable value.
- `rhodolite-wasm-abi`: adds the interim rule that a callable-typed public
  parameter or return value is rejected by the Wasm backend with a
  source-spanned diagnostic, since the backend does not yet generate the
  handle-based invoke export CAB-030 will add.

## Impact

- `src/typecheck.rs`: `check_and_lower` gains public-export awareness (a set
  of canonical public function names, sourced from `module::LoadedProgram`);
  `lower_ret` and the callable-shape checks it shares with
  `check_signature_shape` change to allow a callable return type for public
  exports only.
- `src/main.rs`: both call sites of `typecheck::check_and_lower` (interpret
  and `build`) pass `loaded.public_exports`; `build()` gains a new
  build-only check (after `requirement::analyze`, alongside the existing
  `errors_for_roots` call) that validates ambient-zero and named-function
  resolution for callable-typed public exports.
- `src/requirement.rs` (or a small new module it owns): new helper that,
  given the checked HIR, the requirement `Analysis`, and the public export
  list, resolves each callable-typed public export's returned value to a
  `CallableId` via the existing `hir::callable_of`/`Bindings` machinery and
  reports a diagnostic when resolution fails or the target's own
  requirements are non-empty.
- `src/wasm_abi.rs`: `param_port`/`result_port` gain an explicit `Callable`
  case that reports a "not yet supported by this Wasm build" diagnostic
  instead of routing into the generic `Port::Rich` owned-data path.
- Tests: new focused typecheck scenarios for allow/reject of public callable
  signatures, a build-time ambient-zero diagnostic test, a closure-at-public-
  boundary diagnostic test, and a `rhodolite build` CLI diagnostic test for
  the interim Wasm-backend rejection.
