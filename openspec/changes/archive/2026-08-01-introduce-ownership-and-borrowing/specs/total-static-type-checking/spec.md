## ADDED Requirements

### Requirement: Successful checking closes ownership safety
After ordinary name and type resolution, the checker SHALL successfully classify
every value access as Copy, move, shared borrow, mutable borrow, or owned
construction and SHALL close all move, provenance, and exclusivity obligations
in every loaded body before requirement analysis, interpretation, or Wasm
support checking begins.

#### Scenario: Uncalled body violates ownership
- **WHEN** an uncalled loaded function contains a use after move
- **THEN** whole-program checking fails before execution

#### Scenario: Later stages receive checked ownership
- **WHEN** checking succeeds
- **THEN** requirement analysis, interpretation, and Wasm support checking may assume every access has a resolved ownership mode
