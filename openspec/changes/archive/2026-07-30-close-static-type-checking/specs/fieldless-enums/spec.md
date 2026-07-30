## MODIFIED Requirements

### Requirement: Known enum values are checked at enum-typed struct fields
The checker SHALL compare every struct construction or field assignment source with its enum-typed destination. It SHALL accept a variant of the expected enum and SHALL reject a variant of a different enum or any source whose concrete type cannot be determined.

#### Scenario: Matching variant in struct construction
- **WHEN** field `rank` is declared as `Rank` and a struct literal gives it `Gold` from `Rank`
- **THEN** checking produces no enum-type diagnostic

#### Scenario: Mismatched variant in struct construction
- **WHEN** field `rank` is declared as `Rank` and a struct literal gives it a variant from another enum
- **THEN** checking fails with a diagnostic identifying the expected and actual enum types

#### Scenario: Matching variant in field assignment
- **WHEN** a receiver is known to have a `rank: Rank` field and code assigns `Gold` from `Rank`
- **THEN** checking produces no enum-type diagnostic

#### Scenario: Mismatched variant in field assignment
- **WHEN** a receiver is known to have a `rank: Rank` field and code assigns a variant from another enum
- **THEN** checking fails with a diagnostic identifying the expected and actual enum types

#### Scenario: Source type cannot be determined
- **WHEN** an enum-typed field receives an expression whose type cannot be determined
- **THEN** checking fails before evaluation
