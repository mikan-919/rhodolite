## Why

MAP-020/MAP-025 (`archive/2026-08-05-add-generic-function-instantiation`,
`archive/2026-08-05-add-generic-trait-resolution`) already lower every
generic call site to an ordinary, fully concrete `hir::Callable` with no
type-parameter leaf anywhere in its signature or body, and both tasks' own
`tests/cli.rs` end-to-end tests already run `identity`/`apply`/a generic
trait method's instantiation to completion through the interpreter
(`generic-function-instantiation`'s own spec even states this as a
Requirement: "A concrete instantiation executes through the ordinary
pipeline", with a passing scenario "the interpreter runs the program and
produces `5`"). MAP-040/MAP-050 layered a wider specialization key and
per-instantiation ambient inference on top without touching the interpreter.

What has never happened is dedicated verification *inside* `src/eval.rs`
itself. Every other HIR consumer that sits on this same "instantiation is
just a `CallableId`" boundary — `src/ownership.rs` (MAP-030,
`generic-ownership-boundary`) and `src/requirement.rs`/`src/ambient_abi.rs`
(MAP-050) — already received its own focused, in-module generic test matrix
proving the claim rather than relying on `tests/cli.rs`/`differential.rs`
alone. `src/eval.rs`'s own `mod tests` (1,900+ lines, dozens of cases) has
zero generic-shaped source in it today; every existing proof that the
interpreter runs a generic instantiation lives one layer up, in
`tests/cli.rs` (stdout assertions) or `src/differential.rs` (interpreter-vs-
Wasm result equality). Neither pins down interpreter-internal behavior the
way `Value`/`Diag`-returning `mod tests` cases do elsewhere in this codebase
(exact drop timing, `Store` leak-freedom via `dispose()`'s
`debug_assert!(self.store.locations.iter().all(Option::is_none))`, ambient
binding contents), and MAP-060's own ROADMAP verification method says so
explicitly: "eval の generic `identity` / `apply` / generic trait method 焦
点テスト" — tests in `eval`, not just at the CLI or differential layer. This
closes that specific, previously-untested layer before MAP-070 builds Wasm
generation on the same boundary.

## What Changes

- Add a focused `src/eval.rs` test matrix (`mod tests`, next to the existing
  named-function-value tests) exercising, directly through
  `Interp::new_checked(...).run(entry)`: `identity<T>` called with two
  different type arguments in one program; `apply<T, U>` bound to a `Copy`
  callback and separately to a non-`Copy` (consuming) callback, the latter
  asserting the argument is moved exactly once and the interpreter's
  `dispose()` leak invariant still holds; a generic trait method
  instantiation (`impl<T> Box<T> for Container { fn wrap<U>(...) }`) called
  at two different type arguments; and a generic function's callback
  requiring an ambient value, executed once with the requirement satisfied
  by an enclosing `with` and once where a callback-free sibling
  instantiation of the same generic declaration needs no ambient at all.
- Document interpreter execution of a generic instantiation as its own
  explicit, tested contract: a new `generic-instantiation-execution`
  capability spec, mirroring how MAP-030 gave the ownership-checking side of
  this same boundary its own `generic-ownership-boundary` capability instead
  of folding it into `generic-function-instantiation`/
  `generic-trait-resolution`.
- No production code change is expected in `src/eval.rs`, `src/hir.rs`, or
  `src/typecheck.rs`: `hir::Callable` (`src/hir.rs:583`) carries no field
  distinguishing an instantiation from a hand-written declaration, and every
  `hir::Call` variant `src/eval.rs::call_expr` (`src/eval.rs:884`) handles —
  `Direct`, `Associated`, `Method`, `Indirect`, `Slot` — resolves to
  `CheckedInterp::call(&mut self, callable: hir::CallableId, ...)`
  (`src/eval.rs:318`), which reads only `self.program.callables[callable]`;
  nothing in `src/eval.rs` branches on how a `CallableId` was produced. This
  was confirmed by hand-running each new scenario against the current
  interpreter before writing this proposal (see design.md Decision 1); if
  any scenario in the matrix turns up a real gap once implemented as a
  permanent test, the fix belongs in this change rather than being deferred.

## Capabilities

### New Capabilities
- `generic-instantiation-execution`: the contract that the HIR interpreter
  runs any generic function or generic trait method instantiation as an
  ordinary, fully concrete callable — indistinguishable from a hand-written
  one — so that the same generic body called with different type arguments
  produces independent results, and a consuming callback's value, drop, and
  ambient-requirement behavior for an instantiation matches exactly what
  `ownership::Plan`/`ambient_abi::Plan` already planned for it.

### Modified Capabilities
(none — `generic-function-instantiation`'s "A concrete instantiation
executes through the ordinary pipeline" Requirement and
`generic-trait-resolution`'s equivalent already state the pipeline-neutral
claim this change verifies at the interpreter's own test layer; neither
spec's Requirement text changes)

## Impact

- `src/eval.rs`: new tests only, in the existing `mod tests` block
  (`src/eval.rs:1396`); no change to `CheckedInterp`, `Interp`, `Store`, or
  any evaluation function.
- `openspec/specs/generic-instantiation-execution/spec.md`: new capability
  spec.
- `ROADMAP.md`: MAP-060 row moves to `done` once merged, unblocking
  MAP-070/MAP-075/MAP-080 (all list MAP-060 as a dependency or
  transitively depend on it).
