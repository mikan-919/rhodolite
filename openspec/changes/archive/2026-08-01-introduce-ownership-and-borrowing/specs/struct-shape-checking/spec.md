## ADDED Requirements

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
