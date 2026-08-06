## Why

MAP-080 shipped `Map<T>` / `impl<T> Map<T> for [T]` together with four
differential fixtures (`map-copy-element`, `map-owned-element`,
`map-empty-array`, `map-ambient-callback`) and a focused unit test proving a
missing ambient provider through `map` names the `map` call in its
reachability path (`mapを通る提供忘れの経路はmapを名指す` in
`src/requirement.rs`). Auditing `src/differential.rs` shows the maintained
corpus already runs those fixtures through both engines
(`maintained_corpus_has_matching_observable_outcomes`) and already checks
every fixture — including the `map` ones — for two-build byte determinism
(`every_fixture_is_byte_deterministic_and_independently_executable`). What is
missing is the one thing `SNAPSHOTS` in `src/differential.rs` does not yet
have an entry for: a pinned Wasm-shape fingerprint representative of generic
`map`. Today `SNAPSHOTS` only pins `scalar-control-flow`, `owned-data`, and
`nested-with`, so a codegen regression in `map`'s generated shape (the
generic dispatch, the trait-object call, the ambient-slot threading, the
array push loop) would pass the differential-equality check silently and
never surface for review. MAP-090 cannot be marked `done` per ROADMAP.md
until that representative snapshot is pinned and the full corpus/coverage
picture for `map` is confirmed against MAP-090's four required categories
(scalar/Copy element, owned data, ambient callback, forgotten provider).

## What Changes

- Pin a Wasm-shape fingerprint for `map-ambient-callback` (the richest
  generic-`map` fixture: generic dispatch + trait method resolution +
  ambient-slot propagation + array codegen in one program) into `SNAPSHOTS`
  in `src/differential.rs`, following the existing `fingerprint()`
  byte-length + FNV-1a hash convention used by the three existing entries.
- Extend the `differential-execution` spec's "Representative module shape is
  pinned" scenario to name generic `map` explicitly alongside scalar,
  owned-data, and ambient, so the contract text matches what the suite
  actually pins.
- No new fixtures, no new language behavior, and no changes to
  `openspec/specs/generic-map/spec.md`: this change audits and closes a
  verification gap in already-implemented, already-differentially-tested
  behavior. It does not touch `Map<T>`, `impl<T> Map<T> for [T]`, or any
  compiler pass.
- Run and record the full MAP-010–MAP-100 common completion gate
  (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`) against the corpus as it stands after the snapshot lands, and
  commit that verified state as the single stable snapshot commit MAP-090
  requires.

## Capabilities

### New Capabilities
(none)

### Modified Capabilities
- `differential-execution`: the "Representative module shape is pinned"
  scenario now names generic `map` as one of the pinned representative
  shapes, matching the `SNAPSHOTS` entry this change adds.

## Impact

- `src/differential.rs`: add one `SNAPSHOTS` tuple entry; no other code
  changes. The fixture bodies, `FIXTURES` registrations, and every other
  test in the file are untouched.
- `openspec/specs/differential-execution/spec.md`: requirement text delta
  (see above).
- `ROADMAP.md`: MAP-090's status flips to `done` once this change's tasks
  are applied and the completion conditions are verified (tracked as a task,
  not part of this proposal's spec surface).
- No runtime, ABI, or type-checking behavior changes; no new dependencies.
