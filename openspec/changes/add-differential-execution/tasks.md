## 1. Differential Harness Foundation

- [ ] 1.1 Add test-only adapters that load a fixture through the production
  check/ownership/requirement/planning path and expose structured interpreter
  and generated-Wasm observations without changing CLI behavior
- [ ] 1.2 Define a named fixture registry with source trees, entry/public probes,
  expected completion, and stable failure/snapshot identities
- [ ] 1.3 Define normalized outcomes for successful scalar and ABI-decoded rich
  values, expected runtime-error classes, test summaries, captured output, and
  ordered final-state probes; make mismatch reports show both outcomes
- [ ] 1.4 Compile fixtures through the production Wasm pipeline, validate each
  artifact independently, and invoke only its public wrappers in `wasmi`
- [ ] 1.5 Add focused unit tests for normalization, error classification,
  fixture loading, ABI result decoding, and mismatch diagnostics
- [ ] 1.6 Run formatting and the full test suite; commit the verified harness
  foundation as a stable snapshot

## 2. Scalar, Control-Flow, and Failure Coverage

- [ ] 2.1 Add scalar fixture cases for arithmetic, integer wraparound,
  conditionals, loops, recursion, early return, multiple public exports, and
  multi-module re-exports
- [ ] 2.2 Add supported expected-runtime-failure fixtures and verify their
  interpreter/Wasm error classes agree without comparing engine-specific text
- [ ] 2.3 Add fixture-level comparison of declared test results and captured
  interpreter output where the current language surface exposes them
- [ ] 2.4 Add small deterministic generated-program enumeration with a bounded,
  named seed/case set and run every accepted generated case through both paths
- [ ] 2.5 Run formatting, focused differential tests, the full test suite, and
  repeated-byte checks; commit the verified scalar corpus snapshot

## 3. Owned Values, Borrows, and Observable Final State

- [ ] 3.1 Add ownership-safe fixture cases for strings, structs, payload enums,
  optionals, arrays, `match`, `for`, `clone`, equality, and deterministic cleanup
- [ ] 3.2 Add shared and mutable explicit-borrow fixtures, using read-only public
  probe functions to compare mutation visibility and final owned state after the
  primary entry returns
- [ ] 3.3 Add consuming-call, field projection, optional fallback, return, and
  loop-exit cases that prove move/drop behavior through observable results
- [ ] 3.4 Retain source-positioned unsupported-form and whole-program ownership
  diagnostics in their existing focused suites rather than treating them as
  executable differential cases
- [ ] 3.5 Run formatting, ownership plan/interpreter/Wasm focused tests, the full
  suite, and byte-determinism checks; commit the verified owned-data corpus

## 4. Methods, Traits, and Ambient Providers

- [ ] 4.1 Add inherent and associated method fixtures covering `self`, `&self`,
  and `&mut self` receiver modes with owned and scalar values
- [ ] 4.2 Add trait implementation and value/type-slot fixtures covering direct
  dispatch, recursive calls, transitive requirement forwarding, and unused-slot
  pruning
- [ ] 4.3 Add value-provision fixtures covering shared, mutable, moved, and
  temporary providers, evaluation-once behavior, cleanup, and post-body probes
- [ ] 4.4 Add multi-provision and nested-`with` fixtures covering outer-context
  observation, partial shadowing/restoration, and provider substitution without
  intermediate forwarding edits
- [ ] 4.5 Run formatting, focused ambient plan and differential tests, the full
  suite, and repeated builds; commit the verified ambient corpus snapshot

## 5. Artifact Regression Gate and Compiled-v1 Completion

- [ ] 5.1 Add reviewable generated-module snapshots for representative scalar,
  owned-data, and trait/ambient fixtures, including required ABI metadata and
  import/start-section invariants
- [ ] 5.2 Rebuild every maintained fixture twice and assert byte-identical output
  alongside independent validation and execution
- [ ] 5.3 Add the canonical production program and provider-substitution program
  to the end-to-end differential corpus, including their declared result and
  final-state probes
- [ ] 5.4 Update README and compiler roadmap to mark compiled v1 reached only
  after the maintained differential gate passes; document how to add a fixture
- [ ] 5.5 Run `cargo fmt --check`, the full Rust suite, strict Clippy, strict
  OpenSpec validation, independent Wasm validation/execution, all snapshots,
  and repository-wide repeated-byte checks
- [ ] 5.6 Review the complete diff for accidental CLI/ABI/source-semantics scope,
  mark verified tasks complete, and commit the archive-ready stable snapshot
