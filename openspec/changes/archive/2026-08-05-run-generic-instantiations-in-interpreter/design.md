## Context

See `proposal.md` for motivation and
`specs/generic-instantiation-execution/spec.md` for the contract. This
picks up where MAP-020/MAP-025 (`archive/2026-08-05-add-generic-function-
instantiation`, `archive/2026-08-05-add-generic-trait-resolution`),
MAP-030 (`archive/2026-08-05-harden-generic-ownership-boundary`), MAP-040
(`archive/2026-08-05-widen-generic-specialization-key`), and MAP-050
(`archive/2026-08-05-infer-generic-callback-ambient-requirements`) left off.

`typecheck::instantiate` (`src/typecheck.rs:2290`) is the single function
that produces a concrete `hir::Callable` for a generic free function or a
generic impl method, substituting solved type arguments through the
signature and body and caching the result by `(GenericFnId, type
arguments, callback binding)` (MAP-040). The produced `hir::Callable`
(`src/hir.rs:583`) has no field recording that it came from a generic
declaration — no `GenericFnId` back-reference, no stored type-argument
list — only `name` (a display string MAP-050 already established is not an
identity), `owner`, `receiver`, `params`, `ret`, and `body`. Every call
kind the interpreter's `call_expr` matches (`src/eval.rs:884`) — `Direct`,
`Associated`, `Method`, `Indirect`, `Slot` — resolves to
`CheckedInterp::call(&mut self, callable: hir::CallableId, recv, args)`
(`src/eval.rs:318`), which reads `self.program.callables[callable]` and
proceeds identically regardless of where that `CallableId` came from.
`CheckedInterp::body_plan` (`src/eval.rs:368`) locates the `ownership::
BodyPlan` for the body being run by matching the `hir::Body`'s address
against `program.bodies` — again with no branch on generic-ness — so an
instantiation's moves, borrows, clones, and drops are read from exactly the
plan `ownership::check` (MAP-030) already computed for it, and its ambient
bindings are resolved from `CheckedAmbient`/`hir::Call::Slot` the same way
for every callable (MAP-050 already proved the requirement/ABI-planning
side of this for non-execution purposes).

Before writing this proposal, every scenario in
`specs/generic-instantiation-execution/spec.md` was hand-verified by
temporarily adding matching cases to `src/eval.rs`'s `mod tests` and
running them: `identity<T>` at two type arguments, a generic trait method
instantiation, `apply<T, U>` with a `move`d non-`Copy` argument (confirmed
`dispose()`'s `debug_assert!(self.store.locations.iter().all(Option::
is_none))` leak check still holds), two different type-argument
instantiations each consuming their own non-`Copy` value, and an
ambient-requiring callback reached through a generic call both with and
without an enclosing `with`. All five passed against the current
interpreter with zero production code changes. This design's job is to
turn that throwaway verification into the change's actual, permanent
deliverable: the test matrix itself, plus the spec that states the
contract explicitly for MAP-070 (Wasm generation, which sits on the exact
same "instantiation is just a `CallableId`" boundary) to cite.

## Goals / Non-Goals

**Goals:**
- Give `src/eval.rs` the same dedicated, in-module generic test coverage
  every other HIR-consuming pass on this boundary already has
  (`src/ownership.rs` since MAP-030, `src/requirement.rs`/`src/ambient_abi.rs`
  since MAP-050): a generic free function called at different type
  arguments, a generic trait method instantiation, a consuming callback
  moving a non-`Copy` value exactly once with the store's leak-freedom
  invariant intact, and an ambient-requiring callback reached through a
  generic call with a callback-free sibling requiring nothing.
- Record the interpreter side of this boundary as an explicit,
  independently citable capability spec.
- Fix any gap the new tests uncover in `src/eval.rs` as part of this
  change, per Decision 1.

**Non-Goals (per ROADMAP.md's task boundary):**
- Any change to Core Wasm generation — MAP-070 owns running an
  instantiation through that pipeline; this task's tests exercise the
  interpreter only (`tests/cli.rs`/`src/differential.rs` already exercise
  Wasm for the same source programs, unchanged by this task).
- Any change to `src/typecheck.rs`'s instantiation/specialization logic, to
  `src/ownership.rs`'s planning, or to `src/requirement.rs`/
  `src/ambient_abi.rs`'s requirement inference — MAP-020/025/030/040/050
  already own those, and Decision 1 argues (and hand-verification
  confirmed) the interpreter needs no new logic to consume their output
  correctly.
- Any new language surface, public API, data representation, or runtime
  contract. `hir::Callable`/`CheckedInterp` already have the shape this
  task relies on.

## Decisions

### 1. Treat interpreter execution as already implemented; scope the change to verification, with a fix-on-discovery fallback

Because `hir::Callable` carries no marker distinguishing an instantiation
from a hand-written declaration, and because every `hir::Call` variant
resolves to the same `CheckedInterp::call(CallableId, ...)` entry point
reading only `program.callables`/`program.bodies`/the `ownership::Plan`,
there is no code path in `src/eval.rs` that *could* special-case a generic
instantiation even if one wanted to — the same structural argument MAP-030
made for `src/ownership.rs` and MAP-050 made for `src/requirement.rs`/
`src/ambient_abi.rs`. This was not left as an inference from reading code
alone: every scenario in the new spec was run against the current
interpreter before this proposal was written (see Context), and all
passed unmodified, including the ownership-boundary edge case most likely
to have broken (a `move`d non-`Copy` argument through a generic consuming
callback, checked against `dispose()`'s store-leak assertion).

**Alternative considered:** add an explicit "is this callable a generic
instantiation" branch or logging hook in `CheckedInterp::call`/`call_expr`
for diagnostic clarity. Rejected — there is nothing for such a branch to
do differently; `hir::Callable` gives it no generic-specific data to act
on, and introducing one would be dead complexity whose only effect is a
larger diff to review, exactly the outcome MAP-050's Decision 1 rejected
for the analogous case in `requirement.rs`/`ambient_abi.rs`.

### 2. New capability spec, not a delta to `generic-function-instantiation`/`generic-trait-resolution`

`generic-function-instantiation`'s existing Requirement "A concrete
instantiation executes through the ordinary pipeline" already states the
pipeline-neutral claim in general terms, with one scenario proving the
interpreter runs `identity(5)`. Extending that Requirement in place (the
approach MAP-050 used for `named-function-values`) was considered, but
this task's scope is wider than one Requirement's worth of scenarios: it
covers a second capability's execution (`generic-trait-resolution`'s
generic trait methods) and a third concern (`named-function-values`'/
MAP-050's ambient inference reaching a generic call, observed at
*execution* time rather than at planning time) — spreading three small
deltas across three existing spec files would fragment one coherent
"interpreter's view of this boundary" contract the same way MAP-030
avoided fragmenting the ownership-side contract across
`generic-function-instantiation` and `generic-trait-resolution`. A single
new `generic-instantiation-execution` capability, parallel in structure and
naming to `generic-ownership-boundary`, keeps the interpreter-execution
contract in one place MAP-070 (the next task on this same boundary) can
cite directly, exactly as this task cites `generic-ownership-boundary`.

**Alternative considered:** fold the new scenarios into
`generic-function-instantiation`'s existing Requirement as additional
scenarios, and add a parallel scenario to `generic-trait-resolution`'s
equivalent Requirement. Rejected for the reason above — two partial deltas
covering one boundary is a worse fit for this repo's established pattern
(one dedicated capability per pass on the generic-instantiation boundary)
than MAP-030 already set.

### 3. The test matrix reuses `src/eval.rs`'s existing test harness, `mod tests`, and `Interp`/`Value` API

Every new test is a `#[cfg(test)]` case added to `src/eval.rs`'s existing
`mod tests` (`src/eval.rs:1396`), using the module's own `run(src, entry)`
helper (parse → `typecheck::check_and_lower` → `ownership::check` →
`Interp::new_checked(&checked).run(entry)`) exactly as every existing test
in that module does. No new test infrastructure, harness, or public API is
introduced. The consuming-callback scenario additionally asserts
`dispose()`'s existing debug-mode leak invariant holds (it already runs
automatically at the end of every `run()` call; the test's job is only to
exercise the non-`Copy` generic path that invariant covers).

**Alternative considered:** add the new coverage to `src/differential.rs`
instead, alongside its existing `generic-instantiation`/
`generic-impl-resolution`/`generic-ambient-callbacks` fixtures. Rejected as
the sole location — `differential.rs` compares interpreter and Wasm
*outcomes* (return values, failure classes) and explicitly says nothing
about interpreter-internal state; it cannot assert the store-leak
invariant or otherwise pin down interpreter-internal drop/ambient
mechanics the way an `eval.rs`-local test can. `differential.rs`'s
existing generic fixtures are left unchanged; this task's tests are a
different, previously-missing layer, not a replacement.

## Risks / Trade-offs

- [Hand-verification before writing this proposal is not the same as the
  permanent, reviewed test suite] → Mitigated the same way MAP-030/MAP-050
  handled this: the Migration Plan below re-adds each scenario as a
  permanent test and re-runs the full suite; if any scenario fails once
  written as a real test (as opposed to the throwaway probe used for this
  proposal), the fix belongs in `src/eval.rs` as part of this change, not
  deferred.
- [A new capability spec for a boundary that turns out to need zero
  production code, mirroring MAP-030's `generic-ownership-boundary`] →
  Accepted; MAP-070 (Core Wasm generation) sits on the same boundary next
  and benefits from a fixed, citable interpreter-side contract instead of
  re-deriving it from this task's design notes, exactly as this task cited
  `generic-ownership-boundary` instead of re-deriving MAP-030's reasoning.

## Migration Plan

1. Add the `identity<T>`-at-two-type-arguments test and the
   `apply<T, U>`-with-two-callback-bindings test to `src/eval.rs::mod
   tests`.
2. Add the generic trait method instantiation test (`impl<T> Box<T> for
   Container { fn wrap<U>(...) }` called at two type arguments).
3. Add the consuming-callback test: a `move`d non-`Copy` argument through
   `apply<T, U>`, asserting the result and that no test-suite-visible
   leak occurs (the existing `debug_assert!` in `dispose()` already fires
   on any regression).
4. Add the ambient-requiring generic callback test (with an enclosing
   `with`) and its callback-free sibling test (no ambient in scope).
5. If any test in 1-4 fails, fix the gap in `src/eval.rs` before
   proceeding, then re-run the full suite.
6. Run `cargo fmt --check` and Clippy with warnings as errors; commit the
   stable, passing state as a single snapshot.
7. Rollback is a plain revert: this change adds tests (and, only if
   necessary, a small `src/eval.rs` fix) with no public API, data
   representation, or runtime contract change.
