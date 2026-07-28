## ADDED Requirements

### Requirement: Match branches on a known fieldless enum
The language SHALL provide a value-producing `match` expression whose subject is a known, non-optional fieldless enum value and whose arms are identified by qualified variants of that enum.

#### Scenario: Select a matching arm
- **WHEN** `rank` contains `Rank::Gold` and a match has arms for `Rank::Bronze` and `Rank::Gold`
- **THEN** the `Rank::Gold` arm is selected and its body value becomes the value of the match expression

#### Scenario: Subject is evaluated once
- **WHEN** the match subject is an expression with an observable effect
- **THEN** evaluation performs that effect exactly once before selecting an arm

#### Scenario: Subject is not a known enum
- **WHEN** the match subject has a known scalar, struct, array, optional, or otherwise non-enum type
- **THEN** checking fails with a diagnostic that match requires a non-optional enum

#### Scenario: Subject type is unknown
- **WHEN** the checker cannot determine the subject's enum type
- **THEN** checking fails instead of deferring arm membership and exhaustiveness to runtime

### Requirement: Match arms are exhaustive and unique
For a match over a known enum, the checker SHALL require the arms to contain every declared variant of that enum exactly once and no variant from another enum.

#### Scenario: Exhaustive arms
- **WHEN** every variant declared by the subject enum appears in exactly one arm
- **THEN** checking produces no exhaustiveness diagnostic

#### Scenario: Missing variant
- **WHEN** one or more variants declared by the subject enum have no arm
- **THEN** checking fails with a deterministic diagnostic listing the missing variants

#### Scenario: Duplicate variant
- **WHEN** the same qualified variant appears in more than one arm
- **THEN** checking fails with a diagnostic identifying the duplicate arm

#### Scenario: Variant from another enum
- **WHEN** an arm names a variant whose enum differs from the subject enum
- **THEN** checking fails with a diagnostic identifying the expected and actual enum types

#### Scenario: Unknown variant
- **WHEN** an arm qualifies a name that is not a declared variant of its enum
- **THEN** loading or checking fails with a diagnostic identifying the unknown variant

#### Scenario: Empty enum
- **WHEN** a known empty enum is matched with zero arms
- **THEN** the arm set is considered exhaustive

### Requirement: Match arms produce a compatible result type
The checker SHALL check every arm body against an expected type supplied by the surrounding expression. Without an expected type, it SHALL use the first inferable arm type as the match result type and SHALL check every other known arm type with the existing assignability rules.

#### Scenario: Expected result type
- **WHEN** a match occurs at a destination with expected type `str` and every arm produces `str`
- **THEN** the match satisfies that destination

#### Scenario: Incompatible arm
- **WHEN** one known arm result is incompatible with the expected or inferred match result type
- **THEN** checking fails with a diagnostic identifying the incompatible arm types

#### Scenario: Result feeds later checking
- **WHEN** a context-free match has an inferable result type and its result is passed to a checked call or operator
- **THEN** that inferred type participates in the existing downstream check

#### Scenario: No inferable arm
- **WHEN** an empty match or a match whose arm results are all outside the inference boundary has no expected type
- **THEN** the checker leaves the match result type unknown without inventing a bottom or union type

### Requirement: Match arms preserve lexical and ambient analysis
Each arm body SHALL be a second-class body with the surrounding lexical and ambient environment, and static requirement analysis SHALL conservatively include requirements from every arm.

#### Scenario: Arm-local binding
- **WHEN** one arm introduces a local binding
- **THEN** that binding is unavailable in sibling arms and after the match expression

#### Scenario: Ambient use in one arm
- **WHEN** any arm directly or transitively uses an ambient slot
- **THEN** the containing function carries that slot requirement even when another arm would be selected at runtime

#### Scenario: Return from an arm
- **WHEN** the selected arm exits through `return`
- **THEN** control returns from the containing function under the existing block return semantics
