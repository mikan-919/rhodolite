## Context

See `proposal.md` for motivation. This picks up where MAP-030
(`archive/2026-08-05-harden-generic-ownership-boundary`), MAP-040
(`archive/2026-08-05-widen-generic-specialization-key`), MAP-050
(`archive/2026-08-05-infer-generic-callback-ambient-requirements`), and
MAP-060 (`archive/2026-08-05-run-generic-instantiations-in-interpreter`)
left off, and sits on the exact same "instantiation is just a `CallableId`"
boundary MAP-060's design explicitly named as MAP-070's foundation.

`typecheck::instantiate` produces a concrete `hir::Callable` per
`(GenericFnId, type_args, callback_key)` (MAP-040). `ambient_abi::
plan_production` walks reachable calls from the production roots and turns
every reached `hir::Callable` — generic instantiation or not — into an
`ambient_abi::Instance` keyed by `(body, providers)`, with its own
`RecordLayout` and per-call `PlannedCall { target, receiver, projection }`
(MAP-050; already proven for generics by `ambient_abi.rs`'s
`generic_の具体化ごとにinstanceが分かれる` and
`同じgeneric具体化への呼び出しは1つのinstanceに畳まれる` tests). Neither
step records anywhere that a given `Instance` came from a generic
declaration.

`src/wasm.rs` never mentions "generic" — a `grep` for `generic`/`Generic`
across the file returns no results. Its two entry points read only from
`Plan`/`ProductionPlan`, with no branch that could distinguish an
instantiation from a hand-written callable:

- `check_support_impl` (`src/wasm.rs:139`) iterates `plan.instances()` and
  checks each instance's body against the v0 supported subset — it resolves
  `instance.key.body` to a `hir::Callable` and walks it exactly like any
  other callable's body.
- `build` (`src/wasm.rs:2654`) allocates exactly one Wasm function per
  `plan.instances()` entry, in the plan's own deterministic order
  (`for (index, (_, instance)) in plan.instances().enumerate()`), and every
  call site inside a body is lowered by looking up `PlannedCall.target`
  (an `InstanceId`) — never by re-resolving a name, `GenericFnId`, or trait
  impl. This is exactly `compile-wasm-traits-and-ambient`'s Decision 2
  ("function indices identical to `InstanceId` order... no emitter lookup by
  callable name, trait ID, or implementation ID is permitted"), which that
  change adopted specifically so a later pass adding more instance kinds
  (this one) would not need to change dispatch.

Concretely, `src/differential.rs` already carries three generic fixtures —
`generic-instantiation`, `generic-impl-resolution`, `generic-ambient-
callbacks` (`src/differential.rs:246-322`) — each of which is compiled
through `wasm::check_support`/`wasm::emit`, independently validated,
executed in a `wasmi` engine, and diffed against the interpreter by
`maintained_corpus_has_matching_observable_outcomes`, plus rebuilt twice and
byte-compared by `every_fixture_is_byte_deterministic_and_independently_
executable`. Both tests currently pass (`cargo test --bin rhodolite
differential::` was run before writing this proposal: 5/5 green, including
these three fixtures). None of the three fixtures carries a non-`Copy`
(owned) value through a generic instantiation — all three use only
`int`/`bool` — so the "owned value final state" leg of MAP-070's completion
criteria is exercised nowhere today.

## Goals / Non-Goals

**Goals:**
- Fix, as an explicit and tested contract (`generic-instantiation-wasm`),
  that the Wasm backend runs a generic function's or generic trait method's
  instantiation exactly as it runs a hand-written callable: one Wasm
  function per reached instance, dispatched only through `PlannedCall.
  target`, with return value, failure classification, and owned-value final
  state matching the interpreter.
- Close the coverage gap the current generic fixtures leave open: a
  non-`Copy` value moved through a generic consuming callback, verified both
  differentially (interpreter vs. Wasm) and for allocator reuse (no leak
  across repeated calls) using this backend's existing bounded-loop/capped-
  page technique.
- Record, with `src/wasm.rs`-local snapshot tests reusing the module's
  existing `instance_signature_snapshot`/`plan_of` helpers, that distinct
  type-argument/callback/provider combinations of one generic declaration
  produce distinct, stable internal signatures and ambient-record field
  orders — the Wasm-specific form of MAP-040's "distinct key → distinct
  instance" guarantee.
- Fix any gap discovered in `src/wasm.rs`/`src/wasm_ambient.rs`/
  `src/wasm_data.rs` while writing the above, per Decision 1's fallback.

**Non-Goals:**
- Any change to `src/typecheck.rs` instantiation/specialization,
  `src/ownership.rs` planning, or `src/requirement.rs`/`src/ambient_abi.rs`
  requirement/ABI planning — MAP-030/040/050 already own those, and this
  task's own investigation (Context) found no branch anywhere downstream of
  `ambient_abi::Plan` that could special-case a generic instantiation even
  if one wanted to.
- Table, `funcref`, closures, vtables, or any new host import. The backend
  already forbids these unconditionally (`compile-wasm-traits-and-ambient`
  Decision 4: "A vtable call was rejected because all selections are
  statically known and ADR-0008 forbids it") — this is not a new constraint
  MAP-070 introduces, only one it must not regress while adding generic
  coverage.
- Any new language surface, public Rhodolite ABI change, or CLI surface
  change.
- General ambient/effect requirement refinement, provider-dependent
  requirement narrowing, or anything else `infer-generic-callback-ambient-
  requirements`'s (MAP-050) own Non-Goals already excluded — this task only
  compiles what that plan already decided to include.

## Decisions

### 1. Treat Wasm generation as already implemented for dispatch and reachability; scope the change to verification plus a fix-on-discovery fallback

Exactly as MAP-060's design argued for the interpreter (and for the same
structural reason: `ambient_abi::Instance` carries no "came from a generic
declaration" marker, and every consumer downstream of it — `check_support_
impl`, `build`'s function-allocation loop, and every call site's
`PlannedCall.target` lookup — reads only `Plan`/`Instance`/`hir::Callable`
fields common to every instance), there is no code path in `src/wasm.rs`
that could special-case a generic instantiation even if one wanted to. This
was not left as an inference from reading code alone: the three existing
generic fixtures in `src/differential.rs` were re-run against the current
backend before writing this proposal (see Context) and all pass, including
byte-determinism and independent-engine execution.

This gives the same shape of task MAP-060 was: write the coverage the
ROADMAP's verification method calls for (Wasm generic-instance snapshot
tests, an owned-value differential fixture, an allocator-reuse test for a
generic consuming callback), and if any of it fails against current
`src/wasm.rs`/`src/wasm_ambient.rs`/`src/wasm_data.rs`, fix the gap in this
change rather than deferring it. Unlike MAP-060, this task's completion
criteria add "no table/`funcref`/closure allocation/new host import" as an
explicit, testable boundary; Decision 4's citation above establishes that
boundary already holds structurally for every instance kind, so no new test
is needed to prove absence of a table/`funcref` — the existing full-suite
Clippy/fmt/differential run already guards it (introducing one would show up
as a new import or table section in every byte-snapshot test).

**Alternative considered:** add an explicit "is this callable a generic
instantiation" branch or comment marker in `check_support_impl`/`build` for
documentation clarity. Rejected for the same reason MAP-060's Decision 1 and
MAP-050's Decision 1 rejected the analogous branch in their own passes:
`Instance`/`hir::Callable` give it no generic-specific data to act on, and
adding one would be dead complexity whose only effect is a larger diff.

### 2. New capability spec `generic-instantiation-wasm`, not a delta to `core-wasm-build`/`wasm-traits-and-ambient`

Mirrors MAP-060's Decision 2, which made the identical choice for the
interpreter (`generic-instantiation-execution`, parallel in naming and
structure to MAP-030's `generic-ownership-boundary`). This task's scope
spans three sub-concerns — generic function Wasm execution, generic trait
method Wasm execution, and ambient-requirement-through-generic-callback Wasm
execution — any one of which would need to land as a partial delta spread
across `core-wasm-build`, `wasm-traits-and-ambient`, and `rhodolite-wasm-abi`
if folded into existing capabilities. A single new capability keeps this
boundary's Wasm-side contract in one place, parallel to and directly citable
alongside `generic-instantiation-execution` (the interpreter's version of
the same contract), which is exactly the reuse `generic-instantiation-wasm`'s
own scenarios lean on for wording precedent.

**Alternative considered:** extend `core-wasm-build`'s existing production-
build Requirements with generic-specific scenarios. Rejected — `core-wasm-
build`'s Requirements describe the CLI/production-root contract in general
terms already satisfied by any reached instance; adding generic-specific
scenarios there would duplicate, rather than reuse, the "any reached
instance" framing already present, and would separate this contract from
its interpreter-side twin (`generic-instantiation-execution`) that MAP-070
is explicitly the next step after, per ROADMAP's task ordering.

### 3. `differential-execution`'s maintained-corpus Requirement is extended (MODIFIED), not left implicit

The Requirement's named feature-family list ("scalar control flow,
owned-data construction and cleanup, explicit shared and mutable borrows,
inherent and trait methods, value and type slots, nested `with` provisions,
provider substitution, and the canonical production program") does not
mention generic instantiation, even though `GENERIC_FILES`/`GENERIC_IMPL_
FILES`/`GENERIC_AMBIENT_FILES` already exist in the corpus and pass. This is
a genuine spec/implementation gap independent of whether MAP-070 needs new
production code: the spec currently understates what the maintained corpus
covers. This task closes that gap by adding "generic function and generic
trait method instantiation (scalar and owned-value cases, with and without
ambient requirements)" to the named family list, matching both the existing
fixtures and the new owned-value fixture this task adds (Decision 4).

**Alternative considered:** leave `differential-execution`'s Requirement
text unchanged, on the theory that "every supported feature family" already
implicitly includes generics since instantiation produces an ordinary
callable. Rejected — the Requirement enumerates specific families by name
for a reason (`### Scenario: Every compiled-v1 feature family is
represented` checks against that exact list), and an enumerated contract
that silently omits a family the corpus already covers is a defect in the
contract, not a detail below spec-level.

### 4. The new owned-value generic fixture is added inline in `src/differential.rs`, alongside the existing `GENERIC_*` constants, not as a new file under `tests/fixtures/`

The existing generic fixtures (`GENERIC_FILES`, `GENERIC_IMPL_FILES`,
`GENERIC_AMBIENT_FILES`) are all short inline `&str` sources in
`differential.rs` itself; `tests/fixtures/wasm-owned-data/*.rd` is reserved
for larger, non-generic "every owned-data construct in one program" style
coverage (`all-constructs.rd` is 79 lines covering structs, enums, optional
fields, indirect recursion, and array move together). A generic owned-value
fixture needs only one generic declaration (e.g. `apply<T, U>` or
`identity<T>`) called once with a `move`d struct or array argument through a
consuming callback — small enough to stay inline and consistent with its
three generic siblings, which this fixture is meant to sit beside in the
`FIXTURES` array (gaining byte-determinism and independent-engine coverage
automatically, with no harness change).

**Alternative considered:** extend one of the existing `GENERIC_*` sources
in place to add a struct-typed call alongside its scalar ones. Rejected —
the existing three fixtures each pin a specific, already-reviewed shape
(plain generic call, generic impl method, generic-plus-ambient); mixing in
owned-value coverage would widen what each fixture's snapshot/comparison is
answerable for. A fourth, narrowly-scoped fixture keeps each fixture's
purpose legible, matching this repo's existing one-fixture-per-concern
granularity (`owned-data` vs. `borrowed-final-state` are already split the
same way for the non-generic case).

### 5. Wasm-level generic instance snapshot tests reuse `instance_signature_snapshot`/`plan_of`, added to `src/wasm.rs`'s existing `mod tests`

`wasm-traits-and-ambient` already established this pattern for method/trait
instances (`instance署名とambient_localの並びは固定される`,
`src/wasm.rs:5691`): build a `ProductionPlan` via the module's own `plan_of`
helper, select instances by `program.show_body(instance.key.body)`, and
assert a sorted, formatted signature/ambient-field list. This task adds one
generic-specific test using a source shaped like `ambient_abi.rs`'s own
`GENERIC_CALLBACK_SRC` (an `apply<T, U>` called once per callback binding,
one under a provider and one without), confirming the resulting instances'
internal signatures and hidden ambient-field orders are stable and that the
slot-free instantiation carries no ambient field — the Wasm-emission analog
of what `ambient_abi.rs`'s `generic_の具体化ごとにinstanceが分かれる` already
proves at the planning level.

The allocator-reuse ("no leak") check for the owned-value generic case is a
second, separate `src/wasm.rs`-local test, reusing the existing bounded-
loop/capped-page technique (`ループで作った文字列は使い回される`,
`src/wasm.rs:4003`): call a generic consuming callback with an owned value
inside a bounded loop, `compile` and `invoke_capped` at a tight page count,
and assert success — a leak would exhaust the page cap and trap.

**Alternative considered:** rely solely on `src/differential.rs`'s new
owned-value fixture for leak coverage. Rejected as the sole source — that
harness's `invoke` is uncapped and its comparison is against the
interpreter's single-call outcome, not against repeated-call memory growth;
it cannot, on its own, distinguish "correct once" from "leaks every call but
still returns the right value once." The `wasm.rs`-local capped-loop test is
the only technique in this codebase that observes that distinction.

## Risks / Trade-offs

- [The Context's "already works" claim rests on manually re-running today's
  three generic fixtures once before writing this proposal, not on a
  permanent test] → Mitigated exactly as MAP-060's design mitigated the
  identical risk: the Migration Plan below turns each verification step into
  a permanent test, and any failure is fixed in this change, not deferred.
- [Wasm module size grows linearly with the number of reached, distinct
  generic instantiation instances] → Accepted; this is the same "one
  function per production instance" cost every existing instance already
  pays (`compile-wasm-traits-and-ambient` Decision 2), and MAP-040 already
  guarantees only *reached* combinations are instantiated at all. No new
  measurement is introduced here; revisit only if a real workload's instance
  count becomes a practical problem, which nothing in this task's scope
  exercises.
- [A new capability spec for a boundary that may need zero production code,
  mirroring MAP-060's `generic-instantiation-execution`] → Accepted for the
  same reason MAP-060 accepted it: MAP-075/080 (the next tasks on the
  generic-`map` chain) benefit from a fixed, citable Wasm-side contract
  instead of re-deriving this task's reasoning.

## Migration Plan

1. Add the owned-value generic fixture to `src/differential.rs` (Decision
   4) and register it in `FIXTURES`; run the full differential suite and fix
   any gap surfaced in `src/wasm.rs`/`src/wasm_ambient.rs`/`src/wasm_data.rs`
   before proceeding (Decision 1's fallback).
2. Add the instance-signature/ambient-field snapshot test for a generic
   declaration called at distinct type-argument/callback/provider
   combinations to `src/wasm.rs::mod tests` (Decision 5).
3. Add the capped-page, bounded-loop allocator-reuse test for a generic
   consuming callback moving an owned value (Decision 5).
4. Update `openspec/specs/differential-execution/spec.md`'s maintained-
   corpus Requirement text (Decision 3) and add the new
   `generic-instantiation-wasm` capability spec's Requirements/scenarios,
   matching what steps 1-3 actually test.
5. Run the full test suite, `cargo fmt --check`, and Clippy with warnings as
   errors; commit the stable, passing state as a single snapshot.
6. Mark MAP-070 `done` in `ROADMAP.md`.
7. Rollback is a plain revert: this change adds tests, two spec deltas, and
   (only if Decision 1's fallback fires) a small, narrowly scoped fix in the
   existing Wasm backend files — no public API, data representation, or
   runtime contract change.
