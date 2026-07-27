## MODIFIED Requirements

### Requirement: Unsupported expression forms remain deferred, not dynamically typed
This capability SHALL leave arrays, optional field access, method calls, and associated-function calls outside its inference boundary. The checker SHALL treat this as a temporary implementation limitation rather than as a language-level wildcard or dynamic type. Core optional expressions `nil` and `??` are governed by the optional-core-type-checking capability and are no longer deferred by this requirement.

#### Scenario: Unsupported expression reaches a compatibility check
- **WHEN** an array expression, optional field access, method call, or associated-function call reaches a check added by this capability
- **THEN** this capability produces no mismatch diagnostic solely because that expression has no inferred type

#### Scenario: No implicit wildcard compatibility
- **WHEN** a later capability adds a type for a previously unsupported expression form
- **THEN** the expression participates in the same exact compatibility rules without a wildcard exception
