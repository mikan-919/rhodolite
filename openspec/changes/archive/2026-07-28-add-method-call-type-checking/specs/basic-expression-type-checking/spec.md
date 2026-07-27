## MODIFIED Requirements

### Requirement: Unsupported expression forms remain deferred, not dynamically typed
This capability SHALL defer expressions outside its own inference boundary without treating them as a language-level wildcard or dynamic type. Arrays, core optional expressions, optional field access, method calls, and associated-function calls are governed by their dedicated capabilities and SHALL participate in ordinary compatibility rules whenever those capabilities infer their types.

#### Scenario: Future unsupported expression reaches a compatibility check
- **WHEN** an expression not yet covered by any inference capability reaches a check added by this capability
- **THEN** this capability produces no mismatch diagnostic solely because that expression has no inferred type

#### Scenario: Dedicated capability supplies a type
- **WHEN** a dedicated capability infers the type of an array, optional expression, method call, or associated-function call
- **THEN** the expression participates in the same destination compatibility rules without a wildcard exception
