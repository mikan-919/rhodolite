## MODIFIED Requirements

### Requirement: Optional field typing requires an optional struct
For every optional field expression, the checker SHALL require its receiver to have a known type `S?`, SHALL require `S` to name a loaded struct, and SHALL require that struct to declare the selected field. The result SHALL be the declared field type with its optional bit set. Failure to determine the receiver type SHALL fail checking.

#### Scenario: Read a non-optional declared field
- **WHEN** `user` has type `User?` and `User` declares `name: str`
- **THEN** `user.?name` passes checking and has type `str?`

#### Scenario: Flatten an optional declared field
- **WHEN** `user` has type `User?` and `User` declares `manager: User?`
- **THEN** `user.?manager` has type `User?` rather than a nested optional

#### Scenario: Missing field
- **WHEN** a known optional struct receiver selects a field absent from its declaration
- **THEN** checking fails with a diagnostic identifying the underlying struct and missing field

#### Scenario: Optional non-struct receiver
- **WHEN** a known optional receiver's underlying type is not a loaded struct
- **THEN** checking fails instead of treating it as a struct

#### Scenario: Non-optional receiver
- **WHEN** `.?field` is used on a known non-optional receiver
- **THEN** checking fails because optional field access requires an optional receiver

#### Scenario: Unknown receiver
- **WHEN** the checker cannot determine the receiver type
- **THEN** checking fails at the receiver instead of deferring field validation
