# optional-field-access

## Purpose

Define explicit, read-only field projection through optional struct values,
including nil propagation, static typing, chaining, and invalid receiver forms.

## Requirements

### Requirement: Optional fields use explicit postfix syntax
The parser SHALL recognize `receiver.?field` as optional field access distinct from ordinary `receiver.field`.

#### Scenario: Parse optional field access
- **WHEN** `.?` and a field identifier follow an expression
- **THEN** the parser produces an optional field-read expression whose receiver is that expression

#### Scenario: Continue an optional field chain
- **WHEN** optional field suffixes are continued across a line according to the existing dotted-postfix continuation rule
- **THEN** the parser preserves the same nested optional field accesses as the single-line form

### Requirement: Optional field evaluation propagates nil
Optional field evaluation SHALL evaluate its receiver exactly once. It SHALL return `nil` when the receiver evaluates to `nil`; otherwise it SHALL read the named field from the receiver struct using ordinary field lookup behavior.

#### Scenario: Nil receiver
- **WHEN** the receiver of `.?field` evaluates to `nil`
- **THEN** evaluation returns `nil` without attempting field lookup

#### Scenario: Present struct receiver
- **WHEN** the receiver evaluates to a struct value that contains the named field
- **THEN** evaluation returns that field value

#### Scenario: Chained evaluation stops at nil
- **WHEN** any receiver in `value.?first.?second` evaluates to `nil`
- **THEN** the remaining optional projections propagate `nil`

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

### Requirement: Optional field results chain and reach existing checks
The optional result of `.?` SHALL be available to subsequent optional projections and all existing destination compatibility checks.

#### Scenario: Chained optional fields
- **WHEN** `user: User?`, `User.profile: Profile`, and `Profile.name: str`
- **THEN** `user.?profile.?name` has type `str?`

#### Scenario: Result reaches fallback
- **WHEN** `user.?name` has type `str?`
- **THEN** `user.?name ?? "unknown"` has type `str`

#### Scenario: Result reaches a destination
- **WHEN** an optional field result is passed, assigned, compared, or returned in a context with an established type
- **THEN** the destination or equality rule compares its inferred optional type without silently removing optionality

### Requirement: Ordinary access rejects known optional receivers
Ordinary `receiver.field` SHALL reject a receiver with known optional type and SHALL direct the user to explicit optional access or prior fallback.

#### Scenario: Ordinary field on optional receiver
- **WHEN** `user` has known type `User?` and the source reads `user.name`
- **THEN** checking fails instead of silently deferring the field read

#### Scenario: Ordinary field after optional result
- **WHEN** an optional field result is followed by ordinary `.field`
- **THEN** checking fails because the intermediate receiver remains optional

### Requirement: Optional field access is read-only
Optional field access SHALL NOT be accepted as an assignment target or as a callable method target.

#### Scenario: Optional field assignment
- **WHEN** source attempts `receiver.?field = value`
- **THEN** parsing fails with a diagnostic that optional field access is read-only

#### Scenario: Optional method invocation
- **WHEN** source attempts `receiver.?method(args)`
- **THEN** parsing fails because optional method invocation is not supported
