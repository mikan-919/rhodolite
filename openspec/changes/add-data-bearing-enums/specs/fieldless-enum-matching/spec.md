## MODIFIED Requirements

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
