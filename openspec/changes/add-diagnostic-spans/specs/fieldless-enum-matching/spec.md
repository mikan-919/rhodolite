## MODIFIED Requirements

### Requirement: Match arms are exhaustive and unique
For a match over a known enum, the checker SHALL require the arms to contain every declared variant of that enum exactly once and no variant from another enum. Each exhaustiveness diagnostic SHALL carry a source position: a diagnostic about a particular arm SHALL span that arm, and a diagnostic about variants that have no arm SHALL span the match expression.

#### Scenario: Exhaustive arms
- **WHEN** every variant declared by the subject enum appears in exactly one arm
- **THEN** checking produces no exhaustiveness diagnostic

#### Scenario: Missing variant
- **WHEN** one or more variants declared by the subject enum have no arm
- **THEN** checking fails with a deterministic diagnostic listing the missing variants, spanning the match expression

#### Scenario: Duplicate variant
- **WHEN** the same qualified variant appears in more than one arm
- **THEN** checking fails with a diagnostic identifying the duplicate arm, spanning that arm

#### Scenario: Variant from another enum
- **WHEN** an arm names a variant whose enum differs from the subject enum
- **THEN** checking fails with a diagnostic identifying the expected and actual enum types, spanning that arm

#### Scenario: Unknown variant
- **WHEN** an arm qualifies a name that is not a declared variant of its enum
- **THEN** loading or checking fails with a diagnostic identifying the unknown variant, spanning that arm

#### Scenario: Empty enum
- **WHEN** a known empty enum is matched with zero arms
- **THEN** the arm set is considered exhaustive
