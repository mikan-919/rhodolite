## MODIFIED Requirements

### Requirement: Match branches on a known fieldless enum
The language SHALL provide a value-producing `match` expression whose subject is a known, non-optional enum value. An arm SHALL be identified either by a qualified variant of that enum or by the catch-all pattern `_`. A qualified arm SHALL select its matching variant; when no qualified arm matches, a catch-all arm SHALL be selected. The selected arm body value SHALL become the value of the match expression.

#### Scenario: Select a matching qualified arm
- **WHEN** `rank` contains `Rank::Gold` and a match has a `Rank::Gold` arm
- **THEN** the `Rank::Gold` arm is selected and its body value becomes the value of the match expression

#### Scenario: Select the catch-all arm
- **WHEN** the subject variant has no matching qualified arm and the match has a catch-all arm
- **THEN** the catch-all arm is selected and its body value becomes the value of the match expression

#### Scenario: Qualified arm takes precedence
- **WHEN** the subject variant has a matching qualified arm and the match also has a catch-all arm
- **THEN** the qualified arm is selected and the catch-all body is not evaluated

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
For a match over a known enum, the checker SHALL consider the arms exhaustive when either every declared variant appears in exactly one qualified arm or a catch-all arm is present. Qualified arms SHALL name variants of the subject enum without duplication. A catch-all arm SHALL be written as `_`, SHALL appear at most once, and SHALL be the final arm. Each exhaustiveness diagnostic SHALL carry a source position: a diagnostic about a particular arm SHALL span that arm, and a diagnostic about variants that have no arm SHALL span the match expression.

#### Scenario: Exhaustive qualified arms
- **WHEN** every variant declared by the subject enum appears in exactly one qualified arm
- **THEN** checking produces no exhaustiveness diagnostic

#### Scenario: Catch-all supplies exhaustiveness
- **WHEN** one or more declared variants have no qualified arm and a final catch-all arm is present
- **THEN** checking considers those variants handled and produces no missing-variant diagnostic

#### Scenario: Missing variant without catch-all
- **WHEN** one or more variants declared by the subject enum have neither a qualified arm nor a catch-all arm
- **THEN** checking fails with a deterministic diagnostic listing the missing variants, spanning the match expression

#### Scenario: Duplicate qualified variant
- **WHEN** the same qualified variant appears in more than one arm
- **THEN** checking fails with a diagnostic identifying the duplicate arm, spanning that arm

#### Scenario: Duplicate catch-all
- **WHEN** more than one catch-all arm appears
- **THEN** checking fails with a diagnostic spanning each catch-all after the first

#### Scenario: Catch-all is not final
- **WHEN** any arm follows a catch-all arm
- **THEN** checking fails with a diagnostic spanning the catch-all arm

#### Scenario: Variant from another enum
- **WHEN** a qualified arm names a variant whose enum differs from the subject enum
- **THEN** checking fails with a diagnostic identifying the expected and actual enum types, spanning that arm

#### Scenario: Unknown variant
- **WHEN** a qualified arm names a name that is not a declared variant of its enum
- **THEN** loading or checking fails with a diagnostic identifying the unknown variant, spanning that arm

#### Scenario: Empty enum
- **WHEN** a known empty enum is matched with zero arms
- **THEN** the arm set is considered exhaustive

## ADDED Requirements

### Requirement: Catch-all introduces no pattern bindings
The catch-all arm SHALL match a remaining variant as a whole without exposing its variant identity or payload values. It SHALL introduce no local binding, and its body SHALL otherwise use the same lexical scope, result-type checking, and conservative ambient-requirement analysis as every other arm.

#### Scenario: Payload is not bound
- **WHEN** a catch-all arm handles a variant that carries payload values
- **THEN** the catch-all body runs without introducing names for those payload values

#### Scenario: Outer underscore-like names are unaffected
- **WHEN** the source uses `_` as the catch-all pattern
- **THEN** `_` is not available as a local binding and does not shadow any local or ambient name

#### Scenario: Catch-all result type is checked
- **WHEN** the catch-all body produces a known type incompatible with the expected or inferred match result type
- **THEN** checking fails with the ordinary incompatible-arm diagnostic

#### Scenario: Catch-all contributes ambient requirements
- **WHEN** the catch-all body directly or transitively uses an ambient slot
- **THEN** the containing function carries that slot requirement even if a qualified arm would be selected at runtime
