# struct-shape-checking

## Purpose

Statically match struct declarations against struct construction so that no
struct value with a wrong shape reaches evaluation. This is the first vertical
slice of type checking: it constrains field *sets*, not field *types*.

## Requirements

### Requirement: Struct declarations have unique field names
The checker SHALL reject a struct declaration that declares the same field name more than once.

#### Scenario: Duplicate declaration field
- **WHEN** a loaded program declares `struct User { rank: Rank, rank: Rank }`
- **THEN** checking fails with a diagnostic that identifies `User` and the duplicate `rank` field

#### Scenario: Distinct declaration fields
- **WHEN** a loaded program declares a struct whose field names are all distinct
- **THEN** that declaration produces no struct-shape diagnostic

### Requirement: Struct literals name a declared struct
The checker SHALL require every struct literal to name a struct declaration in the loaded, module-resolved program.

#### Scenario: Declared struct literal
- **WHEN** a struct literal names a declared struct
- **THEN** the checker uses that declaration as the literal's required shape

#### Scenario: Unknown struct literal
- **WHEN** a struct literal names no loaded struct declaration
- **THEN** checking fails with a diagnostic that identifies the unknown name

#### Scenario: Non-struct declaration used as a struct
- **WHEN** a struct literal names a loaded trait, effect, or function instead of a struct
- **THEN** checking fails with a diagnostic that identifies the name as not being a struct

### Requirement: Struct literals provide exactly the declared fields
The checker SHALL require a struct literal to provide every declared field exactly once and SHALL reject fields absent from the declaration. Field order SHALL NOT affect validity.

#### Scenario: Exact field set in declaration order
- **WHEN** a struct literal provides each declared field once in declaration order
- **THEN** the literal produces no struct-shape diagnostic

#### Scenario: Exact field set in a different order
- **WHEN** a struct literal provides each declared field once in an order different from the declaration
- **THEN** the literal produces no struct-shape diagnostic

#### Scenario: Missing field
- **WHEN** a struct literal omits one or more declared fields
- **THEN** checking fails with a diagnostic that identifies each missing field

#### Scenario: Extra field
- **WHEN** a struct literal provides a field absent from the declaration
- **THEN** checking fails with a diagnostic that identifies the extra field

#### Scenario: Duplicate literal field
- **WHEN** a struct literal provides the same field more than once
- **THEN** checking fails with a diagnostic that identifies the duplicate field

### Requirement: Bare struct names represent only zero-field values
The checker SHALL permit an unshadowed struct name as a value only when its declaration has zero fields.

#### Scenario: Bare zero-field struct
- **WHEN** an expression refers to an unshadowed, declared zero-field struct by name
- **THEN** the expression produces no struct-shape diagnostic

#### Scenario: Bare struct with required fields
- **WHEN** an expression refers to an unshadowed struct with one or more declared fields by name
- **THEN** checking fails with a diagnostic requiring an explicit struct literal

#### Scenario: Local binding shadows a struct name
- **WHEN** a parameter, `let` binding, loop binding, or `self` shadows a struct name
- **THEN** references to that local binding are not treated as bare struct values

### Requirement: Shape checking precedes analysis and evaluation
The CLI SHALL run struct-shape checking after module name resolution and before requirement analysis or evaluation, and SHALL NOT evaluate a program with struct-shape diagnostics.

#### Scenario: Invalid code is unreachable at runtime
- **WHEN** an invalid struct literal appears in a loaded function body that is not called
- **THEN** the CLI still reports the struct-shape error and exits unsuccessfully without evaluation

#### Scenario: Valid module-resolved program
- **WHEN** all loaded struct declarations, literals, and bare struct values satisfy the shape rules
- **THEN** the CLI continues to requirement analysis and evaluation with its existing behavior

### Requirement: Structs are owned values
A struct literal SHALL construct an owned value. Struct fields SHALL be stored
as owned subvalues unless declared `indirect`; constructing, moving, cloning,
and dropping the struct SHALL recursively follow the ownership of its fields.

#### Scenario: Struct binding owns fields
- **WHEN** a struct literal initializes an owned local
- **THEN** that local owns every non-borrowed field value

#### Scenario: Struct move is not an alias
- **WHEN** an owned struct moves to another binding
- **THEN** the destination becomes its sole owner and the source cannot observe later changes

### Requirement: Field access follows value mode
Reading a Copy field SHALL copy it. Plainly reading a non-Copy field SHALL
shared-borrow it. `&mut place.field` SHALL mutably borrow the field when its
containing place is mutably accessible. `move place.field` SHALL consume the
entire containing struct, return that field as owned, drop its other owned
fields, and leave no partial-move state.

#### Scenario: Non-Copy field read borrows
- **WHEN** a string field is read without `move`
- **THEN** the result is borrowed and the struct remains owned and initialized

#### Scenario: Mutable field access
- **WHEN** `&mut user.name` is formed from a mutable owner without a conflicting borrow
- **THEN** the field may be changed through exclusive access

#### Scenario: Consuming projection
- **WHEN** `move user.name` extracts an owned field
- **THEN** `user` as a whole is consumed and its other fields are dropped

### Requirement: Field assignment preserves ownership
Field assignment SHALL require exclusive access, drop the old owned field value,
and move or copy the compatible right-hand value according to its Copy status.
It SHALL never implicitly clone a non-Copy value.

#### Scenario: Replace owned field
- **WHEN** a mutable struct field receives a compatible non-Copy owned local
- **THEN** the old field is dropped and ownership of the new value moves into the field
