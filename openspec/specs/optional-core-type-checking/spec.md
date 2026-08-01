# optional-core-type-checking

## Purpose

Give Rhodolite's existing optional annotations, `nil`, and fallback operator a static core: `nil` and present `T` values are contextual at `T?` destinations, and `T? ?? T` safely produces `T` while preserving short-circuit evaluation.

## Requirements

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

### Requirement: Present values inject into optional destinations
At a typed destination, the checker SHALL accept an inferable non-optional value of type `T` when the destination expects the same nominal type `T?`. This contextual injection SHALL apply to parameters, returns, struct fields, field assignments, and established local reassignments. It SHALL NOT change the expression's inferred type, permit `T?` where `T` is expected, or make `T` and `T?` equal.

#### Scenario: Present value reaches an optional destination
- **WHEN** an inferable value has type `T` and a parameter, return, struct field, field assignment, or established local expects `T?`
- **THEN** destination compatibility succeeds and the value remains inferred as `T`

#### Scenario: Optional value reaches a required destination
- **WHEN** an inferable value has type `T?` and a destination expects non-optional `T`
- **THEN** checking fails with the ordinary destination mismatch diagnostic

#### Scenario: Optionality remains exact for equality
- **WHEN** equality compares a known `T` value with a known `T?` value
- **THEN** checking fails because contextual destination injection does not apply to equality operands

### Requirement: Optionals own present values
`T?` SHALL own its present `T` value. Moving, cloning, and dropping an optional
SHALL respectively transfer, structurally duplicate, or release that present
value. An optional of a borrowed type SHALL remain unsupported in this version.

#### Scenario: Present value enters optional
- **WHEN** an owned `T` initializes `T?`
- **THEN** ownership of the value transfers into the optional

### Requirement: Coalescing follows its ownership mode
For Copy `T`, `optional ?? fallback` SHALL produce a copied `T`. For non-Copy
`T`, plain `optional ?? fallback` SHALL produce a shared borrow from the present
value or fallback. `move optional ?? fallback` SHALL consume the optional and
produce an owned present value, evaluating and taking the fallback only when
the optional is empty.

#### Scenario: Borrowed coalesce
- **WHEN** a non-Copy present optional is coalesced without `move`
- **THEN** the result borrows its content and the optional remains initialized

#### Scenario: Consuming coalesce
- **WHEN** a non-Copy optional is coalesced with `move`
- **THEN** the result is owned and the source optional is consumed

#### Scenario: Fallback remains short-circuited
- **WHEN** a consuming coalesce finds a present value
- **THEN** the fallback is not evaluated or moved
