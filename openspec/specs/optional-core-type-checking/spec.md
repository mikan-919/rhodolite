# optional-core-type-checking

## Purpose

Give Rhodolite's existing optional annotations, `nil`, and fallback operator a static core: `nil` is contextual rather than a wildcard, and `T? ?? T` safely produces `T` while preserving short-circuit evaluation.

## Requirements

### Requirement: Nil is contextually compatible only with optional types
The checker SHALL accept `nil` when an existing use site expects `T?` and SHALL reject `nil` when that site expects non-optional `T`. A bare `nil` SHALL NOT independently infer a nominal type or establish a flow-sensitive binding type.

#### Scenario: Nil in an optional destination
- **WHEN** an optional parameter, return, struct field, field assignment, or typed local receives `nil`
- **THEN** checking succeeds at that compatibility site

#### Scenario: Nil in a non-optional destination
- **WHEN** a non-optional parameter, return, struct field, field assignment, or typed local receives `nil`
- **THEN** checking fails with a diagnostic identifying the expected type and `nil`

#### Scenario: Bare nil binding
- **WHEN** a local is initialized with bare `nil`
- **THEN** the binding remains unresolved and a later assignment does not establish a flow-sensitive nominal type

### Requirement: Nil equality uses the other operand as context
The checker SHALL allow equality between `nil` and a known optional value, SHALL reject equality between `nil` and a known non-optional value, and SHALL continue to infer `bool` for the equality result.

#### Scenario: Compare optional value with nil
- **WHEN** either equality operand is `nil` and the other has known type `T?`
- **THEN** checking succeeds and the expression has type `bool`

#### Scenario: Compare non-optional value with nil
- **WHEN** either equality operand is `nil` and the other has known non-optional type `T`
- **THEN** checking fails with an incompatible-operand diagnostic

#### Scenario: Compare nil with nil
- **WHEN** both equality operands are `nil`
- **THEN** checking succeeds and the expression has type `bool`

### Requirement: Fallback unwraps a known optional value
For fallback expression `left ?? right`, the checker SHALL require a known left operand to have type `T?`, SHALL require a value-producing right operand to have exact non-optional type `T`, and SHALL infer non-optional `T` for the result.

#### Scenario: Matching fallback value
- **WHEN** the left operand has known type `T?` and the right operand has known type `T`
- **THEN** checking succeeds and the fallback expression has type `T`

#### Scenario: Non-optional left operand
- **WHEN** the left operand has known non-optional type `T`
- **THEN** checking fails because `??` requires an optional left operand

#### Scenario: Incompatible fallback value
- **WHEN** the left operand has known type `T?` and the right operand has a known type other than non-optional `T`
- **THEN** checking fails with a diagnostic identifying the expected and actual fallback types

#### Scenario: Literal nil gets context from fallback
- **WHEN** the left operand is literal `nil` and the right operand has known non-optional type `T`
- **THEN** checking succeeds and the fallback expression has type `T`

#### Scenario: Unknown optional source remains deferred
- **WHEN** the left operand is outside the current inference boundary and is not literal `nil`
- **THEN** this capability produces no operand mismatch and does not infer a fallback result type

### Requirement: Return may terminate the fallback branch
A direct `return` expression on the right side of `??` SHALL be checked against the enclosing function return type and SHALL satisfy the fallback branch without having to produce the left optional's inner type.

#### Scenario: Return fallback
- **WHEN** the left operand has known type `T?` and the right operand is a direct `return value`
- **THEN** the return value is checked normally and the fallback expression has type `T`

#### Scenario: Invalid returned value
- **WHEN** a direct return fallback carries a value incompatible with the enclosing function return type
- **THEN** checking fails with the ordinary return-type diagnostic

### Requirement: Fallback preserves short-circuit runtime behavior
Evaluation of `left ?? right` SHALL evaluate `left` first, evaluate `right` only when the left value is `nil`, and otherwise return the non-`nil` left value unchanged.

#### Scenario: Non-nil left value
- **WHEN** the evaluated left operand is not `nil`
- **THEN** the evaluator returns it without evaluating the right operand

#### Scenario: Nil left value
- **WHEN** the evaluated left operand is `nil`
- **THEN** the evaluator evaluates and returns the right operand
