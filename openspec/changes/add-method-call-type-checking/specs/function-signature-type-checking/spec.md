## MODIFIED Requirements

### Requirement: Unsupported call forms remain outside signature checking
The checker SHALL limit this capability to direct top-level function calls. Method and associated-function calls SHALL be governed by the dedicated `method-call-type-checking` capability instead of being left without an inferred signature.

#### Scenario: Method call
- **WHEN** a call uses a field receiver such as `value.method()`
- **THEN** this capability leaves the call to `method-call-type-checking`, which resolves and checks it when the receiver or slot is known

#### Scenario: Associated-function call
- **WHEN** a call uses a path such as `Type::make()`
- **THEN** this capability leaves the call to `method-call-type-checking`, which resolves and checks a known type or slot path
