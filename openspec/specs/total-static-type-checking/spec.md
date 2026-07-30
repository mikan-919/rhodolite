# total-static-type-checking

## Purpose

Define the whole-program guarantee that successful static checking leaves no expression type or callable reference unresolved before analysis and evaluation.

## Requirements

### Requirement: Successful checking is total
The checker SHALL assign a concrete language type to every value-producing expression in every loaded function, method, and test body. It SHALL classify control-flow expressions that do not produce a value separately from value types. If either classification is impossible, checking SHALL fail before requirement analysis or evaluation.

#### Scenario: Every loaded body is checked
- **WHEN** a loaded declaration contains an expression whose type cannot be determined
- **THEN** checking fails even when that declaration is not called at runtime

#### Scenario: Control flow exits without a value
- **WHEN** a branch exits through `return`
- **THEN** the checker treats that branch as non-value-producing without inventing a user-visible type

#### Scenario: Successful program reaches later stages
- **WHEN** checking succeeds
- **THEN** requirement analysis and evaluation may assume that every reachable value-producing expression has a concrete type

### Requirement: Successful checking resolves every call
The checker SHALL resolve every direct call, method call, and associated-function call in every loaded body to exactly one declared signature. An unknown receiver, absent target, ambiguous target, or incompatible receiver form SHALL fail checking.

#### Scenario: Direct call resolves
- **WHEN** a direct function call names one loaded function
- **THEN** checking records a successful resolution to that declaration

#### Scenario: Member call cannot resolve
- **WHEN** the receiver type or selected member cannot be determined
- **THEN** checking fails before evaluation

#### Scenario: Ambiguous member call
- **WHEN** more than one declaration remains eligible for a call
- **THEN** checking fails with an ambiguity diagnostic

### Requirement: Unknown is an implementation state, not a successful result
The checker MAY use an internal unknown state while traversing expressions and collecting diagnostics, but SHALL NOT report overall success while any expression or call remains unknown. Unknown SHALL NOT be a user-visible dynamic or wildcard type.

#### Scenario: Earlier error causes an unknown dependent expression
- **WHEN** an expression cannot be typed because one of its inputs already produced a diagnostic
- **THEN** checking may suppress a redundant dependent diagnostic but still fails overall

#### Scenario: Unknown remains without an earlier cause
- **WHEN** checking finishes with an unknown expression that has no earlier diagnostic explaining it
- **THEN** checking emits a diagnostic for that expression

### Requirement: Type failures remain source-positioned
Every diagnostic introduced to close a previously deferred type or call resolution SHALL identify the source expression whose type or target could not be determined.

#### Scenario: Unknown expression in another module
- **WHEN** an unresolved expression occurs in a loaded dependency module
- **THEN** the diagnostic renders against that module and spans the unresolved expression
