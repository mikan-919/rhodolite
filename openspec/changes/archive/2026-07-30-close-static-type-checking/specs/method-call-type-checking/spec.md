## MODIFIED Requirements

### Requirement: Calls resolve with the language's declared candidate rules
The checker SHALL resolve every call by callee form. A dot call on a concrete receiver SHALL select by concrete type and method name across inherent and trait implementations. A dot call on an ambient slot SHALL select the named slot trait's declaration. A `Type::function` call SHALL select by concrete type and function name, and a `slot::function` call SHALL select the named slot trait's declaration. Concrete-type lookup SHALL require exactly one named candidate before validating whether its receiver form matches the call syntax. Failure to determine the receiver type or a unique candidate SHALL fail checking.

#### Scenario: Concrete receiver selects a unique method
- **WHEN** a concrete receiver's type has exactly one implementation candidate with the called name
- **THEN** the checker resolves the dot call to that candidate

#### Scenario: Ambient slot selects its trait
- **WHEN** a dot or path call begins with an unshadowed ambient slot
- **THEN** the checker resolves the call only against that slot's declared trait

#### Scenario: Concrete type selects a unique associated function
- **WHEN** a declared concrete type has exactly one implementation candidate with the called path name
- **THEN** the checker resolves the path call to that candidate

#### Scenario: Unknown member
- **WHEN** a known concrete type or ambient slot has no candidate with the called name
- **THEN** the checker reports that the member does not exist

#### Scenario: Ambiguous concrete member
- **WHEN** a concrete type has more than one implementation candidate with the called name and no slot trait selects one
- **THEN** the checker reports that the member's trait cannot be determined

#### Scenario: Receiver type cannot be determined
- **WHEN** a dot-call receiver has no concrete type and is not an ambient slot
- **THEN** checking fails at the receiver instead of deferring resolution

### Requirement: Resolved calls enforce declared signatures
The checker SHALL compare a resolved call's explicit argument count and every argument type with the selected signature. Argument compatibility SHALL use the existing directional destination rule, including `T` to `T?` injection but not `T?` to `T`. Failure to determine an argument type SHALL fail checking.

#### Scenario: Matching resolved call
- **WHEN** a resolved method or associated-function call has the declared number of compatible arguments
- **THEN** the checker produces no call-signature diagnostic

#### Scenario: Wrong argument count
- **WHEN** a resolved call has fewer or more explicit arguments than its selected signature declares after excluding `self`
- **THEN** the checker reports the member and expected and actual argument counts

#### Scenario: Wrong argument type
- **WHEN** an argument is incompatible with its resolved parameter type
- **THEN** the checker reports the member, argument position, expected type, and actual type

#### Scenario: Argument type cannot be determined
- **WHEN** the checker cannot determine a resolved call argument's type
- **THEN** checking fails at that argument

### Requirement: Resolved call results carry return types
The checker SHALL assign the selected signature's effective return type to every resolved method or associated-function call and SHALL expose it to bindings and all expression, assignment, argument, and return checks. A selected signature without a return annotation SHALL have effective return type `unit`.

#### Scenario: Method result feeds optional fallback
- **WHEN** a resolved method returns `T?` and its result is the left operand of `??` with a compatible `T` fallback
- **THEN** the checker infers the fallback expression as `T`

#### Scenario: Associated function result feeds field access
- **WHEN** a resolved associated function returns a declared struct type and the result is used as a field receiver
- **THEN** the checker validates the field against that struct declaration

#### Scenario: Resolved result reaches an existing mismatch
- **WHEN** a resolved call's return type is incompatible with a destination or operand
- **THEN** the existing compatibility check reports the mismatch

#### Scenario: No return annotation
- **WHEN** a resolved member has no return annotation
- **THEN** the call result has type `unit`
