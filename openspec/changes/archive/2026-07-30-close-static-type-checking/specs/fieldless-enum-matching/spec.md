## MODIFIED Requirements

### Requirement: Match arms produce a compatible result type
The checker SHALL check every value-producing arm body against an expected type supplied by the surrounding expression. Without an expected type, it SHALL use the first concrete value-producing arm type as the match result type and SHALL check every other value-producing arm with existing assignability rules. A value-producing match with no expected type and no concrete arm result type SHALL fail checking. An empty match over an empty enum MAY be classified as non-value-producing.

#### Scenario: Expected result type
- **WHEN** a match occurs at a destination with expected type `str` and every value-producing arm produces `str`
- **THEN** the match satisfies that destination

#### Scenario: Incompatible arm
- **WHEN** one arm result is incompatible with the expected or inferred match result type
- **THEN** checking fails with a diagnostic identifying the incompatible arm types

#### Scenario: Result feeds later checking
- **WHEN** a context-free match has a concrete result type and its result is passed to a checked call or operator
- **THEN** that type participates in the downstream check

#### Scenario: No inferable arm
- **WHEN** a value-producing match has no expected type and no arm with a concrete result type
- **THEN** checking fails instead of leaving the match result unknown

#### Scenario: Empty enum match
- **WHEN** an empty enum is matched with zero arms
- **THEN** the checker classifies the expression as non-value-producing

### Requirement: Qualified match arms require boolean guards
A qualified variant pattern MAY be followed by `if` and a guard expression before its arm body. The checker SHALL require every guard expression to have concrete type `bool`; a non-boolean or unresolved guard SHALL fail checking at the guard expression.

#### Scenario: Boolean guard
- **WHEN** a qualified arm has a guard whose type is `bool`
- **THEN** checking succeeds and arm selection uses that boolean value

#### Scenario: Non-boolean guard
- **WHEN** a qualified arm has a guard whose type is not `bool`
- **THEN** checking fails with a diagnostic spanning the guard expression

#### Scenario: Guard type cannot be determined
- **WHEN** the checker cannot determine a qualified arm guard's type
- **THEN** checking fails before evaluation
