# fieldless-enum-matching

## Purpose

Branch on a fieldless enum by variant instead of stacking equality tests, and
make the checker require that every declared variant is handled — either by its
own qualified arm or by a final catch-all arm. The selected arm's value becomes
the value of the expression, so a match is an ordinary value-producing
expression rather than a statement-only construct.

## Requirements

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
Each arm body SHALL be a second-class body with the surrounding lexical and ambient environment. A payload pattern SHALL extend that environment only for its own arm body: each named pattern element SHALL shadow an outer local, declaration, or slot of the same name, while `_` SHALL introduce no name. Pattern bindings and bindings created in an arm body SHALL be unavailable in sibling arms and after the match expression. Static requirement analysis SHALL conservatively include requirements from every arm while respecting those arm-local bindings.

#### Scenario: Arm-local binding
- **WHEN** one arm introduces a local binding in its pattern or body
- **THEN** that binding is unavailable in sibling arms and after the match expression

#### Scenario: Pattern binding shadows an outer name
- **WHEN** a payload pattern binds the same name as an outer local, declaration, or ambient slot
- **THEN** references in that arm body resolve to the payload binding without changing resolution outside the arm

#### Scenario: Discard does not shadow
- **WHEN** a payload pattern uses `_`
- **THEN** `_` is not available as a local and does not change resolution of any outer name

#### Scenario: Ambient use in one arm
- **WHEN** any arm directly or transitively uses an ambient slot that is not shadowed by its payload pattern
- **THEN** the containing function carries that slot requirement even when another arm would be selected at runtime

#### Scenario: Payload name shadows an ambient slot
- **WHEN** a payload pattern binds the same name as an ambient slot and the arm body uses that name
- **THEN** that use does not create an ambient requirement for the shadowed slot

#### Scenario: Return from an arm
- **WHEN** the selected arm exits through `return`
- **THEN** control returns from the containing function under the existing block return semantics

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
