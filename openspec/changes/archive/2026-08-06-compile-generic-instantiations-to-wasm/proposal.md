## Why

MAP-070 is the last unfinished link in the generic-execution boundary chain:
MAP-030 (ownership), MAP-040 (specialization key), MAP-050 (ambient
inference), and MAP-060 (HIR interpreter) each already established, and
tested, that a generic function's or generic trait method's instantiation is
an ordinary `hir::Callable`/`ambient_abi::Instance` with no marker
distinguishing it from a hand-written declaration. Nothing has yet fixed, as
an explicit and tested contract, that the Core Wasm backend (`src/wasm.rs`)
honors that same boundary: that it emits one direct-called Wasm function per
*reached* instantiation instance, with no table, `funcref`, closure
allocation, or new host import, and that its return value, failure
classification, and owned-value final state agree with the interpreter for
every instance a generic call reaches. `src/differential.rs` already carries
three generic fixtures (`generic-instantiation`, `generic-impl-resolution`,
`generic-ambient-callbacks`) that build and pass through the Wasm pipeline
today, but they are scalar-only (`int`/`bool`) and the `differential-execution`
spec's maintained-corpus Requirement does not name generic instantiation as a
covered feature family — so today's passing state is unverified as a
contract and untested for the owned-value (non-`Copy`) case the ROADMAP
completion criteria explicitly call out.

## What Changes

- Add a new capability spec, `generic-instantiation-wasm`, that fixes the
  Wasm-generation side of the generic-instantiation boundary as an explicit,
  tested contract: reachability-only instance generation, deterministic
  direct calls per distinct type-argument/callback/provider instance, no
  table/`funcref`/closure allocation/new host import, and interpreter-parity
  for return value, failure classification, and owned-value final state.
- Extend `differential-execution`'s maintained-corpus Requirement to name
  generic function and generic trait method instantiation (including
  callback-bound ambient requirements) as a covered feature family, closing
  the gap between the spec text and the fixtures that already exist.
- Add a non-`Copy` (owned struct/array) generic fixture to the differential
  corpus, exercising a `move`d value through a generic consuming callback,
  compiled and run through both the interpreter and an independent Wasm
  engine — the one scenario shape today's scalar-only generic fixtures do
  not cover and MAP-070's completion criteria explicitly require
  ("所有値の最終状態が一致する").
- Add `src/wasm.rs`-local snapshot tests (using the module's existing
  `instance_signature_snapshot`/`plan_of` test helpers, the same pattern
  `wasm-traits-and-ambient` used for method/trait instances) that pin generic
  instantiation's internal function signature, hidden ambient-record field
  order, and direct call target across distinct type-argument, callback, and
  provider combinations of the same generic declaration.
- Fix any gap `src/wasm.rs`/`src/wasm_ambient.rs`/`src/wasm_data.rs` turn out
  to have for a generic instantiation as part of this change, discovered
  while writing the tests above (see design.md Decision 1 for why none is
  expected, and the fallback if one is found).
- Mark MAP-070 `done` in `ROADMAP.md` once the above lands and the full test
  suite, `cargo fmt --check`, and warnings-as-errors Clippy pass.

## Capabilities

### New Capabilities
- `generic-instantiation-wasm`: the Core Wasm backend's contract for
  generic function and generic trait method instantiations — reachable-only
  instance generation, deterministic direct calls with no table/`funcref`,
  no new host import, and interpreter-matching return value, failure class,
  and owned-value final state.

### Modified Capabilities
- `differential-execution`: the maintained-corpus Requirement's named
  feature-family list is extended to include generic function and generic
  trait method instantiation (scalar and owned-value cases, with and without
  ambient requirements), matching fixtures that already exist plus the new
  owned-value fixture this change adds.

## Impact

- `src/wasm.rs`: new `mod tests` cases (instance-signature/ambient-record
  snapshots for generic instantiations); production code changes only if a
  real gap surfaces while writing them (none is expected — see design.md
  Decision 1).
- `src/differential.rs`: extend the maintained corpus with an owned-value
  generic fixture; no change to the fixture-running harness itself.
- `src/wasm_ambient.rs`, `src/wasm_data.rs`: touched only if the new tests
  uncover a gap in ambient projection or clone/drop glue specific to a
  generic instantiation's instance.
- `openspec/specs/generic-instantiation-wasm/spec.md` (new),
  `openspec/specs/differential-execution/spec.md` (modified).
- `ROADMAP.md`: MAP-070 status `in-progress` → `done`.
- No public Rhodolite ABI, source syntax, or CLI surface change.
