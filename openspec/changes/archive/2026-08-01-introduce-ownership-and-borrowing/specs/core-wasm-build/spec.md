## ADDED Requirements

### Requirement: Wasm builds require ownership-safe programs
The Wasm build pipeline SHALL complete whole-program ownership and borrow
checking before production-root planning and reachability-sensitive Wasm support
checking. Reachable borrowed values or non-scalar owned values SHALL remain
unsupported by the current scalar backend, while ownership errors in any loaded
body SHALL fail the ordinary whole-program check even when unreachable.

#### Scenario: Unreachable ownership error
- **WHEN** an unreachable loaded declaration uses a value after move
- **THEN** the Wasm build fails during whole-program ownership checking

#### Scenario: Reachable data remains a target limitation
- **WHEN** an ownership-safe production root reaches an owned array or struct
- **THEN** the build reports the existing source-positioned unsupported Wasm-target diagnostic

#### Scenario: Scalar program remains buildable
- **WHEN** an ownership-safe program uses only Copy scalar values in reachable production code
- **THEN** the current Core Wasm lowering remains available and deterministic
