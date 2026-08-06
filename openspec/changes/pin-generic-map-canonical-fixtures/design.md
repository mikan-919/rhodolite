## Context

`src/differential.rs` already runs 23 fixtures through both the checked-HIR
interpreter and a freshly generated Core Wasm module, asserts observable
outcomes match (`maintained_corpus_has_matching_observable_outcomes`), and
asserts every fixture — including all four `map-*` ones — produces
byte-identical Wasm across two independent `prepare()` calls
(`every_fixture_is_byte_deterministic_and_independently_executable`). A
separate `SNAPSHOTS: &[(&str, &str)]` table additionally pins a fingerprint
(`{byte_len}:{fnv1a_hash}`) for three fixtures — `scalar-control-flow`,
`owned-data`, `nested-with` — so that an *unintentional* shape change in one
of those representative categories fails loudly with a diff to review,
instead of silently passing because interpreter and Wasm still agree with
each other. `map-*` fixtures are not in that table. See proposal.md - Why.

This design covers only closing that one gap. See proposal.md - What
Changes for the full list of edits.

## Goals / Non-Goals

**Goals:**
- Decide which existing `map-*` fixture is the single representative
  snapshot MAP-090 asks for, and record the rationale.
- Confirm, by inspection of `src/differential.rs` and `src/requirement.rs`,
  that MAP-090's four required categories (scalar/Copy element, owned data,
  ambient callback, forgotten provider) are already represented for `map`
  before this change adds anything — this change should only need to add a
  snapshot entry, not new fixtures.

**Non-Goals:**
- Changing `Map<T>`, `impl<T> Map<T> for [T]`, or any typecheck/ownership/
  requirement/wasm codegen behavior. MAP-080 already implemented and
  differentially tested the feature; this change only pins verification
  artifacts.
- Building executable Wasm test bodies. `differential.rs`'s `Outcome` struct
  already documents, in its own comment, that "the language currently has
  neither stdout nor executable Wasm test bodies" and that both engines'
  `declared_tests` fields are compared as a normalized contract that makes
  that absence explicit rather than hiding it. `examples/canonical.rd`
  already contributes one declared `test` to the corpus via the
  `canonical-production` fixture, so the comparison path is already
  exercised at least once. Nothing about generic `map` changes that
  contract, and building real Wasm test execution is a separate, much
  larger effort with no MAP-090 mandate.
- Adding a CLI-level (`tests/cli.rs`) end-to-end "missing provider through
  `map`" test analogous to `examples/missing_handler.rd`'s. The scenario
  ("A missing provider through `map` reports a path naming `map`") is
  already a passing, focused test —
  `mapを通る提供忘れの経路はmapを名指す` in `src/requirement.rs` — at the
  layer (`requirement::analyze`) that actually owns the reachability-path
  wording. `examples/missing_handler.rd`'s CLI test exists to keep a
  *documentation example* from bit-rotting; `map`'s missing-provider case
  has no equivalent standalone example file, so there is nothing for a CLI
  test to guard against rotting. Adding one would duplicate coverage
  without closing a gap.

## Decisions

### Decision 1: Pin `map-ambient-callback`, not `map-copy-element` or `map-owned-element`
`SNAPSHOTS`'s three existing entries each anchor a different feature
dimension (plain scalar control flow, owned-data construction/cleanup,
nested ambient `with` provision). The equivalent "richest" `map` fixture is
`map-ambient-callback`: it is the only one of the four `map-*` fixtures that
exercises generic dispatch (`Map<T>` → `impl<T> Map<T> for [T]`), a second,
independent generic trait dispatch (`Tick` → `Counter`) for the ambient
slot, ambient-slot threading through `map`'s callback argument, *and* the
array-push loop body, all in one compiled module. Pinning it gives the
snapshot test the largest blast radius for catching an accidental codegen
shape change anywhere in the generic-`map` lowering path. `map-copy-element`
and `map-owned-element` are strict subsets of that shape (no ambient slot);
pinning one of them in addition would not catch anything
`map-ambient-callback`'s snapshot doesn't already cover, so this change
pins exactly one entry, matching MAP-090's "代表的な" (singular
representative) wording and the existing three-entry precedent (each prior
entry is also a single representative, not an exhaustive set).

**Alternatives considered:**
- Pin `map-copy-element` (closest analog to `scalar-control-flow`): rejected
  because it duplicates coverage `map-ambient-callback` already subsumes,
  and MAP-090 asks for one representative snapshot, not category parity
  with the pre-existing three entries.
- Pin all four `map-*` fixtures: rejected as unnecessary churn-detection
  surface for no additional regression-catching value beyond what
  `map-ambient-callback` alone provides, and inconsistent with how the
  three existing entries were scoped (one per dimension, not one per
  fixture).

### Decision 2: Compute the fingerprint by running the test, not by hand
Every existing `SNAPSHOTS` entry's hash was produced the same way: add the
tuple with a placeholder, run
`cargo test representative_module_shapes_match_reviewed_snapshots`, let the
assertion failure report the actual computed fingerprint, and paste that
value in. This change follows the identical procedure (see tasks.md) rather
than reimplementing `fingerprint()`'s FNV-1a hash by hand, which would be
error-prone and adds no value over letting the existing test compute it.

## Risks / Trade-offs

- [Risk] Pinning `map-ambient-callback`'s snapshot makes any future,
  intentional change to `map`'s codegen (or to the generic dispatch /
  ambient-slot lowering it shares with other features) require a reviewed
  snapshot update, same as the three existing pinned fixtures already do.
  → Mitigation: this is the intended behavior (the snapshot test's failure
  message already says "review and intentionally update its snapshot"); no
  new risk is introduced beyond what the existing mechanism already accepts
  for its three current entries.
- [Risk] `map-ambient-callback`'s snapshot could still miss a codegen change
  confined only to `map-copy-element`'s or `map-owned-element`'s exact
  shape (e.g. a `Copy`-vs-move element bug that happens not to alter the
  ambient-heavy fixture's bytes). → Mitigation: `map-copy-element` and
  `map-owned-element` remain in `FIXTURES` and stay covered by
  `maintained_corpus_has_matching_observable_outcomes` (semantic
  differential equality) and
  `every_fixture_is_byte_deterministic_and_independently_executable` (byte
  determinism); only the *reviewed-snapshot* layer is scoped to one
  representative, matching the existing precedent for scalar/owned/ambient.
