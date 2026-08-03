## Why

Rhodolite now has two independent execution paths for its compiled-v1 surface: the
checked-HIR interpreter and an import-free Core Wasm module executed by a separate
engine. Individual backend fixtures compare selected programs today, but there is
no maintained, common contract that continuously detects a semantic drift between
the two paths.

## What Changes

- Add a maintained differential-execution harness that runs the same supported
  programs through the HIR interpreter and a freshly built Core Wasm module in an
  independent Wasm engine.
- Compare the observable outcome of each path: returned values, successful or
  failing completion, stdout, test results, runtime-error classification, and
  declared final owned/mutable state where a fixture exposes it.
- Define a corpus spanning scalar control flow, owned data and cleanup, explicit
  borrows, methods, trait implementations, slots, and nested `with` provisions,
  including the canonical program and provider substitution.
- Add deterministic generated-module snapshots, small generated-program cases,
  and repeated-build byte comparisons to the maintained verification path.
- Keep unsupported source forms, public borrowed signatures, host imports,
  start sections, fuzzing, and arbitrary external Wasm engines outside this
  change.

## Capabilities

### New Capabilities

- `differential-execution`: a maintained test contract that proves supported
  Rhodolite programs have matching interpreter and Core Wasm outcomes.

### Modified Capabilities

<!-- No existing language or artifact contract changes; this change adds a
     verification capability around the already supported compiled-v1 surface. -->

## Impact

- Primary work is in the Rust test harness and maintained source fixtures, with
  small read-only test seams in interpreter/Wasm support modules only where an
  observable result needs structured comparison.
- The existing `wasmi` dev dependency remains the independent execution engine;
  no public CLI syntax, source semantics, Wasm ABI, or runtime dependency changes.
- README and compiler roadmap will record compiled v1 only after the differential
  corpus, generated-module snapshots, and byte-determinism checks are established.
