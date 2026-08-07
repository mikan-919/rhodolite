## Why

MAP-040 (`archive/2026-08-05-widen-generic-specialization-key`) widened
`instantiate()`'s cache to `(GenericFnId, type arguments, callback binding)`
specifically so that two calls to the same generic declaration with the same
type arguments but different concrete callbacks — e.g. `apply<int, int>`
bound to `fetch` versus bound to `sendEmail` — become distinct physical
`hir::Callable`s, each able to carry its own ambient requirement set. Both
`generic-function-instantiation`'s and this repo's own code comments name the
reason explicitly: "MAP-050 が具体化ごとに違う ambient の要求を貼れるように
するため" (`src/typecheck.rs:2283`). `generic-function-instantiation`'s spec
still lists this as an open Non-Goal: "Inferring ambient/effect requirements
through a generic function's callback parameters on a per-callback basis
(MAP-050)."

Nobody has yet exercised this end-to-end. `src/requirement.rs`'s existing
`(BodyId, Bindings)`-keyed analysis and `src/ambient_abi.rs`'s existing
`InstanceKey`-keyed ABI planner were both written, and tested, only against
non-generic callback specialization (one `apply` `Callable`, several virtual
`(BodyId, Bindings)` specializations layered on top by `hir::callee_bindings`/
`hir::callable_of`). Neither has a single test where the *callee itself* is a
generic instantiation. Reading both modules end to end against MAP-040's
model shows the core walk (`scan`/`scan_body` in `requirement.rs`,
`walk`/`plan_call` in `ambient_abi.rs`) needs no new logic — it is driven
entirely by finished-HIR shape (params, `Let`s, call arguments), which is
identical for a generic instantiation and a hand-written function once
monomorphization has run — but one real, verifiable bug already exists
between them: `requirement::Analysis`'s human-facing summary
(`Analysis.reqs`/`Analysis.order`, which backs the CLI's "推論された要求"
listing at `src/main.rs:308`) keys purely by `program.show_body(id)`, and
`instantiate()` copies a generic declaration's plain name into every one of
its instantiations unchanged (`src/typecheck.rs:2346`: `name,` reusing
`decl.name.clone()`). Two reachable instantiations of the same generic
function — different type arguments and/or different callbacks — silently
collide on that one string key, and the later one in `program.bodies`
overwrites the earlier one's line, dropping its requirement set from the
listing entirely.

This change makes the "requirement inference already generalizes to
generics" claim a tested fact rather than an inference from reading code, and
fixes the one collapsing bug the reading surfaced, closing MAP-050 and
unblocking MAP-060/MAP-070 (both list MAP-050 as a dependency).

## What Changes

- Add a focused test matrix to `src/requirement.rs` and `src/ambient_abi.rs`
  proving, for generic call sites (`Call::Direct`/`Call::Indirect` through a
  generic free function's callable-typed parameter — the `apply<T, U>` shape
  used throughout MAP-020/025/040's own fixtures): requirement analysis and
  ambient-ABI planning already produce one precise, correct result per
  distinct `(type arguments, callback binding)` specialization, including
  when a callback is forwarded through a chain of nested generic calls, and
  including that a callback-free specialization of a generic function picks
  up none of a sibling specialization's slots.
- Fix `requirement::analyze_hir`'s summary-map construction (`src/requirement.rs`,
  the loop building `Analysis.reqs`/`Analysis.order`) so that two physically
  distinct reachable bodies that render to the same display name (always true
  for two instantiations of one generic declaration, since `instantiate()`
  deliberately keeps `Callable.name` as the plain declared name) each keep
  their own entry instead of the later one overwriting the earlier one. This
  is a display/diagnostics-only fix: `Analysis::requirements(body, bindings)`
  (the exact, `BodyId`-keyed API `ambient_abi::plan` and the interpreter use)
  and `unsatisfied_for`/`errors_for_roots` (entry-point diagnostics, whose
  roots are never generic instantiations) are already unaffected, and
  `ambient_abi::Plan::render()` already disambiguates every instance with a
  numeric `instance#N` prefix.
- After archiving, hand-edit `openspec/specs/generic-function-instantiation/spec.md`'s
  `## Non-Goals` to remove the bullet naming MAP-050 ("Inferring ambient/effect
  requirements through a generic function's callback parameters on a
  per-callback basis"), since this change fulfills it — the same
  outside-the-delta touch-up MAP-040 used for the Non-Goal it fulfilled
  (`archive/2026-08-05-widen-generic-specialization-key/tasks.md` §5).
- No change to `src/typecheck.rs`, `src/ownership.rs`, the HIR shape, or the
  public ABI: MAP-040 already produces the physically-separate instantiations
  this change's analysis distinguishes; this change only tests and reports
  on them correctly.

## Capabilities

### New Capabilities
(none)

### Modified Capabilities
- `named-function-values`: the "Ambient requirements flow through indirect
  calls" requirement is clarified to state explicitly that it applies per
  concrete generic instantiation — a distinct `(type arguments, callback
  binding)` specialization of a generic function is exactly one more callable
  whose own indirect calls and requirement set this requirement already
  covers — with new scenarios for a generic callback specialization, a
  callback forwarded through nested generic calls, a callback-free
  specialization of the same generic declaration staying clean, and a
  missing-provider diagnostic whose path includes both a generic helper and
  the selected callback. A new requirement covers the CLI/listing surface
  reporting every reachable specialization of a generic declaration
  separately instead of collapsing them under one shared display name.

`generic-function-instantiation` has no Requirement-level change (the
instantiation mechanism this change relies on already shipped in MAP-040);
only its prose Non-Goals list changes, handled as a post-archive touch-up
(see Impact), the same way MAP-040 handled the Non-Goal it fulfilled.

## Impact

- `src/requirement.rs`: `analyze_hir`'s summary-map construction loop (the
  `public`/`order` building code, ~line 558-577); new tests only elsewhere
  (`scan`, `scan_body`, `hir::callable_of`/`callee_bindings`/`resolve_bindings`
  are exercised, unmodified, by the new tests).
- `src/ambient_abi.rs`: no production code change; new tests exercising
  `plan_hir_for_test`/`Plan`/`Instance`/`RecordLayout` against generic
  instantiations.
- `openspec/specs/named-function-values/spec.md`,
  `openspec/specs/generic-function-instantiation/spec.md`: spec text only.
- `ROADMAP.md`: MAP-050 row moves to `done` once merged (per this repo's
  roadmap-tracking convention), unblocking MAP-060/MAP-070.
