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
- **THEN** the expression participates in the same destination compatibility rules without a wildcard exception

### Requirement: Known assignments preserve exact types
The checker SHALL compare inferable struct-literal field values, field-assignment values, and ordinary-binding reassignments with their established destination types using directional compatibility. Exact nominal identity and optionality SHALL be compatible. A non-optional `T` value SHALL also be compatible with a destination of the same nominal type `T?`. An optional `T?` value SHALL NOT be compatible with a non-optional `T` destination.

#### Scenario: Matching struct-literal field value
- **WHEN** a struct literal supplies an inferable value whose type exactly matches the declared field type
- **THEN** the checker produces no field-type diagnostic

#### Scenario: Present value initializes an optional field
- **WHEN** a struct field is declared as `T?` and its inferable initializer has type `T`
- **THEN** checking succeeds without changing the initializer's inferred type

#### Scenario: Mismatched struct-literal field value
- **WHEN** a struct literal supplies an inferable value incompatible with the declared field type
- **THEN** checking fails with a diagnostic identifying the struct, field, expected type, and actual type

#### Scenario: Matching field assignment
- **WHEN** a known struct field receives an inferable value compatible with its declaration
- **THEN** the checker produces no assignment-type diagnostic

#### Scenario: Mismatched field assignment
- **WHEN** a known struct field receives an inferable value incompatible with its declaration
- **THEN** checking fails with a diagnostic identifying the field, expected type, and actual type

#### Scenario: Matching local reassignment
- **WHEN** an ordinary binding with an established type is reassigned an inferable compatible value
- **THEN** the checker produces no reassignment-type diagnostic

#### Scenario: Mismatched local reassignment
- **WHEN** an ordinary binding with an established type is reassigned an inferable incompatible value
- **THEN** checking fails with a diagnostic identifying the binding, expected type, and actual type

#### Scenario: Optional value does not flow into a required destination
- **WHEN** a field or established local has type `T` and receives an inferable value of type `T?`
- **THEN** checking fails with the ordinary assignment mismatch diagnostic

#### Scenario: Assignment source is temporarily unknown
- **WHEN** an assignment source is outside this capability's inference boundary
- **THEN** this capability defers compatibility checking

#### Scenario: Binding initializer is temporarily unknown
- **WHEN** a binding's initializer is outside this capability's inference boundary
- **THEN** later assignments do not establish a flow-sensitive type for that binding
