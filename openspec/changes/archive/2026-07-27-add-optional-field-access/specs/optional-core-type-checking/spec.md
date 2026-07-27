## ADDED Requirements

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
