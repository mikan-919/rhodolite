## ADDED Requirements

### Requirement: Reachable owned data programs lower to Core Wasm
The Core Wasm backend SHALL lower reachable ownership-safe uses of `str`, owned structs, payload enums, optionals, arrays, declared indirect values, field access and assignment, ownership modifiers, `clone()`, `??`, `match`, and `for`. It SHALL preserve the runtime behavior specified by their existing language capabilities and by `wasm-owned-data-values`.

#### Scenario: Owned data program executes
- **WHEN** a production root reaches strings, an owned struct, a payload enum, an optional, and an array using supported operations
- **THEN** the generated module validates and produces the same result as the reference interpreter

#### Scenario: Canonical data subset builds
- **WHEN** the canonical program is evaluated without reaching trait or ambient operations that remain outside this change
- **THEN** its owned-data expressions no longer cause an unsupported Wasm-target diagnostic

## MODIFIED Requirements

### Requirement: Unsupported checks are reachability-sensitive
All loaded code SHALL still pass the ordinary whole-program static and ownership checks. Wasm-target support SHALL then be checked only for specialization instances reachable from production roots. A reachable expression, call form, value type, runtime record, or public boundary outside the supported scalar and owned-data subset SHALL cause a source-positioned diagnostic naming the unsupported construct. Unsupported unreachable code SHALL not block production emission.

#### Scenario: Unsupported declaration is unreachable
- **WHEN** a loaded but unreachable function uses traits, ambient calls, `with`, aggregate-stored borrows, or another feature outside the supported Wasm subset
- **THEN** the declaration remains statically checked but does not prevent a supported production build

#### Scenario: Unsupported construct is reachable
- **WHEN** a production root reaches a construct outside the supported scalar and owned-data subset
- **THEN** the build fails before publishing the artifact and points to the reached construct

#### Scenario: Owned data construct is reachable
- **WHEN** a production root reaches ownership-safe strings, structs, enums, optionals, arrays, matching, or iteration
- **THEN** the construct is checked and lowered rather than rejected merely for using a non-scalar value

### Requirement: Wasm builds require ownership-safe programs
The Wasm build pipeline SHALL complete whole-program ownership and borrow checking before production-root planning, layout, and reachability-sensitive Wasm support checking. Reachable owned values and internal checked borrows SHALL lower according to the checked ownership modes and drop plans. Borrowed public signatures and aggregate-stored borrows SHALL remain unsupported, while ownership errors in any loaded body SHALL fail the ordinary whole-program check even when unreachable.

#### Scenario: Unreachable ownership error
- **WHEN** an unreachable loaded declaration uses a value after move
- **THEN** the Wasm build fails during whole-program ownership checking

#### Scenario: Reachable owned data is accepted
- **WHEN** an ownership-safe production root reaches an owned array or struct using checked internal borrows
- **THEN** the backend plans its layout and emits it instead of reporting the former non-scalar target limitation

#### Scenario: Public borrow remains rejected
- **WHEN** a selected public function has a borrowed parameter or result
- **THEN** the build fails before emission with a source-positioned public ABI diagnostic

#### Scenario: Scalar program remains buildable
- **WHEN** an ownership-safe program uses only Copy scalar values in reachable production code
- **THEN** the existing scalar Core Wasm lowering remains available and deterministic
