## ADDED Requirements

### Requirement: Local mutability is declared
`let` SHALL create an immutable binding and `let mut` SHALL create a mutable
binding, with either form retaining the existing optional type annotation.
Reassignment, mutable field access, and creation of `&mut` SHALL require a
mutable owned binding or an existing mutable reference.

#### Scenario: Immutable binding cannot be reassigned
- **WHEN** code assigns a second value to a binding declared with `let`
- **THEN** checking fails and points to the immutable declaration

#### Scenario: Mutable annotated binding
- **WHEN** code declares `let mut user: User = value`
- **THEN** compatible reassignment and mutable borrowing are permitted

### Requirement: Non-Copy binding follows ownership
Initializing another local from a non-Copy owned local SHALL move ownership.
Initializing from `&place` or `&mut place` SHALL create the corresponding borrow,
and initializing from `clone()` SHALL create a distinct owned value.

#### Scenario: Binding a borrow
- **WHEN** code declares `let view = &user`
- **THEN** `view` has type `&User` and does not own or copy `user`

#### Scenario: Mutable borrow needs mutable source
- **WHEN** code declares `let edit = &mut user` from immutable `user`
- **THEN** checking rejects the borrow
