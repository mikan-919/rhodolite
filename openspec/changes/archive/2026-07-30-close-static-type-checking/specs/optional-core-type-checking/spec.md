## MODIFIED Requirements

### Requirement: Nil is contextually compatible only with optional types
The checker SHALL accept `nil` only when an expected type `T?` is available from its use site and SHALL reject `nil` when that site expects non-optional `T`. A bare `nil` SHALL NOT independently infer a nominal type, and an unannotated local initialized with bare `nil` SHALL fail checking.

#### Scenario: Nil in an optional destination
- **WHEN** an optional parameter, return, struct field, field assignment, or annotated local receives `nil`
- **THEN** checking succeeds at that compatibility site and the expression is contextualized as that optional type

#### Scenario: Nil in a non-optional destination
- **WHEN** a non-optional parameter, return, struct field, field assignment, or annotated local receives `nil`
- **THEN** checking fails with a diagnostic identifying the expected type and `nil`

#### Scenario: Bare nil binding
- **WHEN** an unannotated local is initialized with bare `nil`
- **THEN** checking fails and recommends an optional local type annotation

### Requirement: Nil equality uses the other operand as context
The checker SHALL allow equality between `nil` and a concrete optional value, SHALL reject equality between `nil` and a concrete non-optional value, and SHALL infer `bool` for a valid equality result. Equality between two uncontextualized `nil` literals SHALL fail because no nominal optional type can be determined.

#### Scenario: Compare optional value with nil
- **WHEN** either equality operand is `nil` and the other has type `T?`
- **THEN** checking succeeds, contextualizes `nil` as `T?`, and gives the expression type `bool`

#### Scenario: Compare non-optional value with nil
- **WHEN** either equality operand is `nil` and the other has non-optional type `T`
- **THEN** checking fails with an incompatible-operand diagnostic

#### Scenario: Compare nil with nil
- **WHEN** both equality operands are uncontextualized `nil`
- **THEN** checking fails because their nominal optional type cannot be determined

### Requirement: Fallback unwraps an optional value
For fallback expression `left ?? right`, the checker SHALL require the left operand to have type `T?`, SHALL require a value-producing right operand to have exact non-optional type `T`, and SHALL infer non-optional `T` for the result. Literal `nil` MAY receive `T?` context from a concrete right operand. Any other failure to determine an operand type SHALL fail checking.

#### Scenario: Matching fallback value
- **WHEN** the left operand has type `T?` and the right operand has type `T`
- **THEN** checking succeeds and the fallback expression has type `T`

#### Scenario: Non-optional left operand
- **WHEN** the left operand has non-optional type `T`
- **THEN** checking fails because `??` requires an optional left operand

#### Scenario: Incompatible fallback value
- **WHEN** the left operand has type `T?` and the right operand has a type other than non-optional `T`
- **THEN** checking fails with a diagnostic identifying the expected and actual fallback types

#### Scenario: Literal nil gets context from fallback
- **WHEN** the left operand is literal `nil` and the right operand has non-optional type `T`
- **THEN** checking contextualizes the left as `T?` and gives the fallback expression type `T`

#### Scenario: Optional source type cannot be determined
- **WHEN** the left operand is not literal `nil` and its type cannot be determined
- **THEN** checking fails at the left operand
