## ADDED Requirements

### Requirement: Enum variants declare positional payload types
An enum variant SHALL declare zero or more positional payload types. A variant without a payload list SHALL remain a fieldless variant with the existing value behavior. Every payload type name SHALL be canonicalized under the same nominal type and module rules as function parameter and struct field types.

#### Scenario: Mixed fieldless and payload variants
- **WHEN** one enum declares a fieldless variant and variants with one or several payload types
- **THEN** all variants belong to that enum and retain their declared payload arity and order

#### Scenario: Payload type from another module
- **WHEN** a variant payload names an imported declared type
- **THEN** loading resolves it to the same canonical type identity used by other type positions

#### Scenario: Optional and array payload types
- **WHEN** a variant payload uses optional, array, or nested array type syntax
- **THEN** its complete type shape and canonical named leaves are retained in the declaration

### Requirement: Qualified variant constructors validate their payload
A call to a qualified payload variant SHALL construct a value of the declaring enum. The checker SHALL require the call to have exactly the declared number of arguments and SHALL check every argument against the corresponding payload type with the existing assignability rules. A payload variant path without a constructor call SHALL not be a value.

#### Scenario: Construct a payload variant
- **WHEN** `Lookup::Found(user)` supplies one `User` value to a variant declared as `Found(User)`
- **THEN** the expression produces a `Lookup` value containing that `User`

#### Scenario: Construct a fieldless variant
- **WHEN** `Lookup::Skipped` refers to a variant with no payload
- **THEN** it keeps producing the existing fieldless `Lookup` value without parentheses

#### Scenario: Wrong constructor arity
- **WHEN** a qualified variant constructor receives fewer or more values than its declaration
- **THEN** checking fails with a diagnostic identifying the expected and actual payload counts

#### Scenario: Wrong constructor payload type
- **WHEN** a constructor argument has a known type incompatible with its declared payload type
- **THEN** checking fails at that argument with a type diagnostic

#### Scenario: Payload variant used without constructing it
- **WHEN** a qualified payload variant path appears as a value without its required argument list
- **THEN** checking fails instead of creating a partial or first-class constructor value

#### Scenario: Non-variant path call
- **WHEN** a qualified call does not identify a declared enum variant
- **THEN** existing associated-function and ambient type-projection resolution remains in effect

### Requirement: Match patterns bind variant payloads
Each match arm for a payload variant SHALL provide one flat pattern element per declared payload position. A pattern element SHALL be either a binding name or `_`. A binding name SHALL have the corresponding declared payload type and SHALL be visible only in that arm body. `_` SHALL discard the corresponding value without introducing a binding.

#### Scenario: Bind one payload
- **WHEN** `Lookup::Found(user)` matches a runtime `Found` value
- **THEN** the arm body can use `user` as the declared payload type and receives the contained value

#### Scenario: Bind several payloads in order
- **WHEN** a variant declares two payload types and its arm binds two names
- **THEN** each name receives the value at the same declaration position

#### Scenario: Discard a payload
- **WHEN** an arm uses `_` for one payload position
- **THEN** matching succeeds and no local name is introduced for that value

#### Scenario: Pattern arity mismatch
- **WHEN** an arm has fewer or more pattern elements than its variant declares
- **THEN** checking fails with a diagnostic identifying the expected and actual binding counts

#### Scenario: Duplicate binding in one pattern
- **WHEN** one arm pattern binds the same name more than once
- **THEN** checking fails instead of silently replacing the earlier payload

#### Scenario: Binding type feeds existing checks
- **WHEN** an arm uses a payload binding in a field read, call, assignment, or return
- **THEN** the binding's declared payload type participates in the existing static checks

### Requirement: Payload enum values preserve existing enum semantics
A payload enum value SHALL retain its enum identity, variant identity, and payload values during evaluation. Matching SHALL evaluate constructor arguments once and SHALL execute only the matching arm. Equality SHALL compare enum identity, variant identity, payload count, and each payload using the existing value equality rules.

#### Scenario: Subject and payload are evaluated once
- **WHEN** construction and matching contain expressions with observable effects
- **THEN** each constructor argument and the match subject is evaluated exactly once

#### Scenario: Only matching arm executes
- **WHEN** a payload enum value is matched by an exhaustive match
- **THEN** only the arm for that value's variant receives payload bindings and executes

#### Scenario: Equal payload enum values
- **WHEN** two enum values have the same enum, variant, and pairwise-equal payloads
- **THEN** equality evaluates to true

#### Scenario: Unequal payload enum values
- **WHEN** two enum values differ in enum identity, variant identity, or any payload
- **THEN** equality evaluates to false

#### Scenario: Existing fieldless program
- **WHEN** a program uses only fieldless enum declarations, qualified variant values, and fieldless match arms
- **THEN** its checking and runtime behavior remain unchanged
