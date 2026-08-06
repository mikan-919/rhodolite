## 1. Confirm existing coverage before adding anything

- [x] 1.1 Confirm `map-copy-element` (`src/differential.rs`) is the `map`
      corpus member for MAP-090's scalar/`Copy`-element category, and that
      it is already registered in `FIXTURES` and passes
      `maintained_corpus_has_matching_observable_outcomes`.
- [x] 1.2 Confirm `map-owned-element` is the `map` corpus member for the
      owned-data category (moved non-`Copy` element, final ownership state
      compared across engines) and is already registered and passing.
- [x] 1.3 Confirm `map-ambient-callback` is the `map` corpus member for the
      ambient-callback category (callback requiring an ambient slot,
      provided around the call) and is already registered and passing.
- [x] 1.4 Confirm `mapを通る提供忘れの経路はmapを名指す`
      (`src/requirement.rs`) is the focused test for the forgotten-provider
      category, asserts the reachability path both contains `"map"` and a
      hop ending in the caller (`mapped`), and passes.
- [x] 1.5 If any of 1.1-1.4 does not already hold, stop and report back
      before continuing — the design assumes all four are already
      implemented and passing from MAP-080; do not add new fixtures or
      tests to backfill a gap without confirming the gap first.

## 2. Pin the representative generic `map` snapshot

- [x] 2.1 In `src/differential.rs`, add a fourth tuple to `SNAPSHOTS` for
      `"map-ambient-callback"` with a placeholder fingerprint (e.g.
      `"0:0000000000000000"`), per design.md Decision 1.
- [x] 2.2 Run
      `cargo test representative_module_shapes_match_reviewed_snapshots`,
      read the actual computed fingerprint out of the assertion failure,
      and replace the placeholder with it, per design.md Decision 2.
- [x] 2.3 Re-run the same test and confirm it now passes.

## 3. Update the spec text to match

- [x] 3.1 Sync the `differential-execution` delta from
      `specs/differential-execution/spec.md` into
      `openspec/specs/differential-execution/spec.md`, replacing the
      "Differential artifacts are independently executable and
      deterministic" requirement so its "Representative module shape is
      pinned" scenario names generic `map` alongside scalar, owned-data,
      and ambient.

## 4. Full verification gate

- [x] 4.1 `cargo test` passes in full (including
      `maintained_corpus_has_matching_observable_outcomes`,
      `every_fixture_is_byte_deterministic_and_independently_executable`,
      and `representative_module_shapes_match_reviewed_snapshots`).
- [x] 4.2 `cargo fmt --check` passes.
- [x] 4.3 `cargo clippy --all-targets -- -D warnings` passes.
- [x] 4.4 Confirm two consecutive `prepare()` runs of `map-ambient-callback`
      (already exercised generically by
      `every_fixture_is_byte_deterministic_and_independently_executable`,
      re-verified here for the specific fixture this change pins) produce
      byte-identical Wasm.
- [x] 4.5 Commit the stable, passing state as a single snapshot.
- [ ] 4.6 Archive this change and mark MAP-090 `done` in `ROADMAP.md`
      (ROADMAP update is a follow-up step, same as prior MAP-0XX changes).
