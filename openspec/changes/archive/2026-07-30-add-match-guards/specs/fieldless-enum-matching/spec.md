## MODIFIED Requirements

### Requirement: Match branches on a known fieldless enum
The language SHALL provide a value-producing `match` expression whose subject
is a known, non-optional enum value. An arm SHALL be identified either by a
qualified variant of that enum or by the catch-all pattern `_`. A qualified arm
whose variant matches SHALL be selected when it has no guard or its guard
evaluates to `true`. When no qualified arm is selected, a catch-all arm SHALL be
selected. The selected arm body value SHALL become the value of the match
expression.

#### Scenario: Select a matching unguarded qualified arm
- **WHEN** `rank` contains `Rank::Gold` and a match has an unguarded `Rank::Gold` arm
- **THEN** the `Rank::Gold` arm is selected and its body value becomes the value of the match expression

#### Scenario: Select a matching guarded qualified arm
- **WHEN** the subject variant matches a qualified arm and that arm's guard evaluates to `true`
- **THEN** the qualified arm is selected and its body value becomes the value of the match expression

#### Scenario: False guard falls through to catch-all
- **WHEN** the subject variant matches a guarded qualified arm and its guard evaluates to `false`
- **THEN** the qualified body is not evaluated and the catch-all arm is selected

#### Scenario: Select the catch-all arm
- **WHEN** the subject variant has no matching qualified arm and the match has a catch-all arm
- **THEN** the catch-all arm is selected and its body value becomes the value of the match expression

#### Scenario: Qualified arm takes precedence
- **WHEN** the subject variant has a matching unguarded qualified arm and the match also has a catch-all arm
- **THEN** the qualified arm is selected and the catch-all body is not evaluated

#### Scenario: Subject is evaluated once
- **WHEN** the match subject is an expression with an observable effect
- **THEN** evaluation performs that effect exactly once before selecting an arm

#### Scenario: Guard is evaluated conditionally and once
- **WHEN** a qualified arm has a guard with an observable effect
- **THEN** evaluation performs that effect exactly once when the arm's variant matches and not at all when it does not match

#### Scenario: Subject is not a known enum
- **WHEN** the match subject has a known scalar, struct, array, optional, or otherwise non-enum type
- **THEN** checking fails with a diagnostic that match requires a non-optional enum

#### Scenario: Subject type is unknown
- **WHEN** the checker cannot determine the subject's enum type
- **THEN** checking fails instead of deferring arm membership and exhaustiveness to runtime

### Requirement: Match arms are exhaustive and unique
For a match over a known enum, the checker SHALL consider an individual variant
exhaustively handled only by an unguarded qualified arm or by a catch-all arm.
A guarded qualified arm SHALL NOT establish exhaustiveness because its guard
may be false. Qualified arms SHALL name variants of the subject enum without
duplication, whether guarded or unguarded. A catch-all arm SHALL be written as
`_`, SHALL have no guard, SHALL appear at most once, and SHALL be the final arm.
Each exhaustiveness diagnostic SHALL carry a source position: a diagnostic
about a particular arm SHALL span that arm, and a diagnostic about variants
that have no unconditional arm SHALL span the match expression.

#### Scenario: Exhaustive qualified arms
- **WHEN** every variant declared by the subject enum appears in exactly one unguarded qualified arm
- **THEN** checking produces no exhaustiveness diagnostic

#### Scenario: Catch-all supplies exhaustiveness
- **WHEN** one or more declared variants have no unguarded qualified arm and a final catch-all arm is present
- **THEN** checking considers those variants handled and produces no missing-variant diagnostic

#### Scenario: Guarded arm does not supply exhaustiveness
- **WHEN** a variant appears only in a guarded qualified arm and no catch-all arm is present
- **THEN** checking fails with a missing-variant diagnostic spanning the match expression

#### Scenario: Missing variant without catch-all
- **WHEN** one or more variants declared by the subject enum have neither an unguarded qualified arm nor a catch-all arm
- **THEN** checking fails with a deterministic diagnostic listing the missing variants, spanning the match expression

#### Scenario: Duplicate qualified variant
- **WHEN** the same qualified variant appears in more than one arm regardless of their guards
- **THEN** checking fails with a diagnostic identifying the duplicate arm, spanning that arm

#### Scenario: Guard on catch-all
- **WHEN** a catch-all pattern is followed by an `if` guard
- **THEN** parsing fails because catch-all guards are not supported

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

### Requirement: Match arms preserve lexical and ambient analysis
Each arm body and guard SHALL use the surrounding lexical and ambient
environment. A payload pattern SHALL extend that environment only for its own
guard and body: each named pattern element SHALL shadow an outer local,
declaration, or slot of the same name, while `_` SHALL introduce no name.
Pattern bindings and bindings created in an arm body SHALL be unavailable in
sibling arms and after the match expression. Static requirement analysis SHALL
conservatively include requirements from every guard and arm body while
respecting those arm-local bindings.

#### Scenario: Arm-local binding
- **WHEN** one arm introduces a local binding in its pattern or body
- **THEN** that binding is unavailable in sibling arms and after the match expression

#### Scenario: Pattern binding is visible to the guard
- **WHEN** a payload pattern binds a name and that arm has a guard
- **THEN** the guard resolves that name to the corresponding payload value

#### Scenario: Pattern binding shadows an outer name
- **WHEN** a payload pattern binds the same name as an outer local, declaration, or ambient slot
- **THEN** references in that arm's guard and body resolve to the payload binding without changing resolution outside the arm

#### Scenario: Discard does not shadow
- **WHEN** a payload pattern uses `_`
- **THEN** `_` is not available as a local and does not change resolution of any outer name

#### Scenario: Ambient use in one arm
- **WHEN** any arm body directly or transitively uses an ambient slot that is not shadowed by its payload pattern
- **THEN** the containing function carries that slot requirement even when another arm would be selected at runtime

#### Scenario: Ambient use in a guard
- **WHEN** any guard directly or transitively uses an ambient slot that is not shadowed by its payload pattern
- **THEN** the containing function carries that slot requirement even when that arm's variant would not match at runtime

#### Scenario: Payload name shadows an ambient slot
- **WHEN** a payload pattern binds the same name as an ambient slot and the arm's guard or body uses that name
- **THEN** that use does not create an ambient requirement for the shadowed slot

#### Scenario: Return from an arm
- **WHEN** the selected arm exits through `return`
- **THEN** control returns from the containing function under the existing block return semantics

## ADDED Requirements

### Requirement: Qualified match arms may carry boolean guards
A qualified variant pattern MAY be followed by `if` and a guard expression
before its arm body. The checker SHALL check an inferable guard expression
against `bool`; a known non-boolean guard SHALL fail checking at the guard
expression. If the guard type is outside the current inference boundary,
evaluation SHALL still require a boolean value at runtime.

#### Scenario: Boolean guard
- **WHEN** a qualified arm has a guard whose inferred type is `bool`
- **THEN** checking succeeds and arm selection uses that boolean value

#### Scenario: Known non-boolean guard
- **WHEN** a qualified arm has a guard whose inferred type is not `bool`
- **THEN** checking fails with a diagnostic spanning the guard expression

#### Scenario: Unknown guard type
- **WHEN** a qualified arm's guard type is outside the current inference boundary
- **THEN** static checking defers the type mismatch and evaluation requires the guard to produce `bool`

#### Scenario: Runtime non-boolean guard
- **WHEN** a deferred guard evaluates to a non-boolean value
- **THEN** evaluation fails with a runtime diagnostic spanning the guard expression
