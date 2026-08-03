## Context

See `proposal.md` for motivation and `specs/differential-execution/spec.md` for
the contract. The project already has a checked-HIR interpreter, Core Wasm
emission from the same checked program, direct `wasmi` execution helpers, and
several focused interpreter/Wasm comparisons. Those checks are distributed
across unit and CLI tests, use different source snippets, and do not normalize
all compiled-v1 observations in one maintained corpus.

`tests/cli.rs` exercises the released CLI process and generated artifacts;
`src/wasm.rs` unit tests can construct checked programs and invoke generated
modules directly. The language exposes no stdout primitive or structured
runtime-error taxonomy today, and the generated module cannot inspect the
interpreter's internal store. Fixture assertions therefore need to state the
observable values and final state that both paths can expose through ordinary
entry/public functions.

## Goals / Non-Goals

**Goals:**

- Give both paths a single, normalized test-only observation model.
- Keep every compared Wasm execution independent of the emitter and retain the
  existing import-free, no-start artifact contract.
- Make fixture coverage explicit, reviewable, deterministic, and cheap enough
  for the normal Rust test suite.

**Non-Goals:**

- Changing Rhodolite syntax, static semantics, the public CLI contract, or the
  Core Wasm ABI.
- Supporting programs that the Wasm backend currently rejects, comparing host
  traps byte-for-byte, or adding a second production interpreter.
- Fuzzing, random program generation, external-engine matrix testing, coverage
  measurement, or runtime instruction tracing.

## Decisions

### 1. Use a fixture registry with one source definition and explicit probes

Add a test-only differential fixture registry. Each fixture owns its source
tree, selected entry/public functions, expected compilation disposition, and
optional probe functions that expose final owned/mutable state as values
crossing the existing Wasm boundary. The fixture name becomes the stable
failure/snapshot key.

This keeps the oracle in source semantics rather than hand-coding expected
machine values. A one-off test per feature was rejected because it lets source
variants, build options, and comparison rules drift. Synthesizing probes from
the HIR was rejected because it would test a transformed program rather than
the maintained source program.

### 2. Normalize only cross-boundary observations

The harness will record a structured `Outcome` for each path: successful scalar
or ABI-decoded rich value, classified expected runtime failure, test summary,
captured CLI output where applicable, and ordered probe results. Interpreter
values are rendered/encoded through the same language-level value contract used
by the fixture; Wasm values are decoded via the existing ABI metadata and
exports. Error comparison uses a small stable class (for example successful,
source diagnostic, division-by-zero, integer-overflow, or engine trap), with
full diagnostics retained in assertion output.

Comparing display strings or raw engine messages was rejected because spans,
paths, and engine wording are deliberately not stable semantics. Reaching into
the interpreter store or Wasm linear memory was rejected because it would bind
the test contract to two unrelated representations. Final mutation and owned
state are instead made observable by fixture-owned probe functions.

### 3. Exercise the production pipeline, then invoke artifacts independently

Each successful fixture will load, type-check, ownership-check, plan, support
check, and emit through the same production sequence as `rhodolite build`; the
resulting bytes are independently validated and instantiated with `wasmi`.
The interpreter side executes the checked program produced by that same loaded
source. Public wrappers/probes, not private specialized instances, are called
from the Wasm engine.

Using only the CLI subprocess was rejected: parsing human output cannot provide
reliable structured values or rich-result comparisons. Calling emitter internals
without planning/support checks was rejected because it bypasses a production
boundary that regressions must cover. `wasmi` remains a dev-only oracle engine;
the generated module shares no execution code with it.

### 4. Separate semantic differential assertions from artifact regression checks

The fixture runner will first assert normalized interpreter/Wasm equality. A
companion artifact suite will snapshot representative validated modules and
rebuild every fixture twice in the same test process, requiring byte equality.
Snapshot updates remain reviewable deliberate changes; byte checks apply to the
whole corpus so unsnapshotted lowering drift is also detected.

Snapshots alone were rejected because valid byte changes can preserve wrong
behavior. Value-only comparisons were rejected because they miss accidental
imports, start sections, wrapper changes, or nondeterministic output.

### 5. Treat unsupported forms separately from differential fixtures

The differential corpus contains only forms accepted by the current Wasm
support boundary. Existing reachability-sensitive unsupported diagnostics remain
their own source-positioned tests. A fixture can assert a shared pre-execution
diagnostic only where both selected paths are intentionally expected to reject
before execution; engine-only unsupported cases are not represented as semantic
equivalence.

This prevents the harness from disguising a missing backend feature as a
successful comparison and keeps the compiled-v1 gate honest.

## Risks / Trade-offs

- **[A shared source fixture can become too broad to diagnose]** → Keep fixtures
  narrowly named by behavior and add one end-to-end canonical fixture rather
  than folding every feature into it.
- **[Probe functions accidentally change ownership/liveness]** → Make probes
  normal public read-only functions called after the entry, and cover them with
  ownership-plan/interpreter assertions before adding them to the differential
  registry.
- **[Engine trap text differs from interpreter diagnostics]** → Compare stable
  classes and keep raw texts only as failure context.
- **[Rich ABI decoding obscures a backend defect]** → Test scalar and rich
  public results separately, retain direct export tests, and snapshot the
  corresponding module metadata.
- **[Corpus runtime grows with every feature]** → Reuse one compiled artifact
  per fixture per run, limit generated-program enumeration deterministically,
  and retain the suite in the default test command.

## Migration Plan

1. Extract or add test-only loading, checked-program, interpreter, ABI decode,
   and independent Wasm-invocation adapters without changing production CLI
   output or source semantics. Pin their normalized outcomes with scalar cases.
2. Establish the fixture registry and add scalar/control-flow plus expected
   runtime-failure fixtures; run the artifacts twice per fixture.
3. Add owned-data, clone/drop, explicit borrow, and final-state probe fixtures,
   followed by methods, trait slots, provider substitution, and nested `with`.
4. Add generated-program enumeration and representative module snapshots;
   finish with the canonical production fixture and update the roadmap to
   compiled v1 only after the full suite passes.

Each slice remains a passing git snapshot. A regression can be rolled back by
reverting its slice; no persisted data, public ABI, or published artifact format
is migrated.
