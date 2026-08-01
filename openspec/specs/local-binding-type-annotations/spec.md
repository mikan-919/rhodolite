# local-binding-type-annotations

## Purpose

Allow a local binding to supply the expected type needed by context-dependent expressions such as bare `nil` and empty array literals.

## Requirements

### Requirement: Local bindings accept an optional type annotation
The language SHALL accept `let name: T = value` wherever an ordinary `let name = value` binding is accepted. The annotation SHALL use the same named, array, and optional type syntax and module resolution rules as parameters, fields, and returns.

#### Scenario: Annotated scalar binding
- **WHEN** source declares `let count: int = 1`
- **THEN** the binding has type `int`

#### Scenario: Annotated optional binding
- **WHEN** source declares `let user: User? = nil`
- **THEN** `nil` is checked with expected type `User?` and the binding has type `User?`

#### Scenario: Annotated empty array
- **WHEN** source declares `let users: [User] = []`
- **THEN** the empty literal is checked with element type `User` and the binding has type `[User]`

#### Scenario: Imported annotation type
- **WHEN** an annotation names an imported type
- **THEN** the name resolves by the same module rules as a parameter annotation

### Requirement: Annotated initializers satisfy the declared type
The checker SHALL compare an annotated binding initializer with the declared type using ordinary directional destination compatibility. Later references and reassignments SHALL use the declared binding type.

#### Scenario: Compatible initializer
- **WHEN** `let user: User? = present_user` receives a `User` value
- **THEN** checking succeeds through the existing present-to-optional injection

#### Scenario: Incompatible initializer
- **WHEN** `let count: int = true` is checked
- **THEN** checking fails at the initializer with expected type `int` and actual type `bool`

#### Scenario: Reassignment follows annotation
- **WHEN** an annotated local is later assigned an incompatible value
- **THEN** checking fails against the originally declared local type

### Requirement: Unannotated bindings still require inferable initializers
An unannotated binding SHALL take its type from its initializer. If that initializer has no independently inferable type, checking SHALL fail and direct the user to add a type annotation or otherwise provide type context.

#### Scenario: Unannotated bare nil
- **WHEN** source declares `let user = nil`
- **THEN** checking fails because the optional nominal type is not known

#### Scenario: Unannotated empty array
- **WHEN** source declares `let users = []`
- **THEN** checking fails because the element type is not known

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
