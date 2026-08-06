## MODIFIED Requirements

### Requirement: Differential artifacts are independently executable and deterministic
Every Wasm artifact used by the differential corpus SHALL validate independently,
instantiate without required imports or an automatic start action, and be
executed only through its exported entry or public wrapper. The suite SHALL
retain deterministic generated-module snapshots for representative programs —
including a scalar, an owned-data, an ambient, and a generic array `map`
program — include small generated-program cases, and build every corpus
member twice to assert byte-identical artifacts.

#### Scenario: Generated artifact is run independently
- **WHEN** a corpus member is compiled for Wasm
- **THEN** an independent validator accepts the module and an independent Core
  Wasm engine invokes its required exports

#### Scenario: Repeated build preserves bytes
- **WHEN** the suite builds the same corpus member twice with identical sources
  and options
- **THEN** the two generated module byte sequences are identical

#### Scenario: Representative module shape is pinned
- **WHEN** a representative scalar, owned-data, ambient, or generic array
  `map` corpus member is intentionally changed
- **THEN** its generated-module snapshot exposes the artifact-shape change for
  review
