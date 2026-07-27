## MODIFIED Requirements

### Requirement: Known ordinary field reads use struct declarations
For an ordinary field expression `receiver.field` whose receiver has a known non-optional type, the checker SHALL require that type to name a loaded struct and SHALL require the struct to declare the field. A valid read SHALL have the field's declared type. An ordinary field expression whose receiver has a known optional type SHALL fail and require explicit optional access or prior fallback.

#### Scenario: Read a declared field
- **WHEN** `user` has known non-optional struct type `User` and `User` declares `rank: Rank`
- **THEN** `user.rank` passes field checking and has type `Rank`

#### Scenario: Chain declared field reads
- **WHEN** each receiver in a non-optional field chain has a known struct type and each field is declared
- **THEN** the checker propagates each declared field type through the chain

#### Scenario: Read a missing field
- **WHEN** a known struct receiver is used with a field absent from its declaration
- **THEN** checking fails with a diagnostic identifying the receiver type and missing field

#### Scenario: Read a field from a known non-struct
- **WHEN** a receiver has a known non-optional type that is not a loaded struct
- **THEN** checking fails instead of treating the receiver as a struct

#### Scenario: Read an ordinary field from an optional receiver
- **WHEN** an ordinary field receiver has known optional type
- **THEN** checking fails and identifies explicit optional access or prior fallback as the valid alternatives

#### Scenario: Receiver type is temporarily unknown
- **WHEN** a field receiver is an expression form outside this capability's inference boundary
- **THEN** this capability defers the receiver and produces no field-type diagnostic

### Requirement: Unsupported expression forms remain deferred, not dynamically typed
This capability SHALL leave arrays, method calls, and associated-function calls outside its inference boundary. The checker SHALL treat this as a temporary implementation limitation rather than as a language-level wildcard or dynamic type. Core optional expressions and optional field access are governed by their dedicated capabilities and are no longer deferred by this requirement.

#### Scenario: Unsupported expression reaches a compatibility check
- **WHEN** an array expression, method call, or associated-function call reaches a check added by this capability
- **THEN** this capability produces no mismatch diagnostic solely because that expression has no inferred type

#### Scenario: No implicit wildcard compatibility
- **WHEN** a later capability adds a type for a previously unsupported expression form
- **THEN** the expression participates in the same exact compatibility rules without a wildcard exception
