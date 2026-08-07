## ADDED Requirements

### Requirement: Function signatures declare ownership modes
Function parameters and results SHALL accept owned `T`, shared `&T`, and mutable
`&mut T` forms. Reference nesting and references inside aggregate type arguments
SHALL remain outside this version. An owned result SHALL move from the returned
place automatically; a borrowed result SHALL carry inferred provenance.

#### Scenario: Three parameter modes
- **WHEN** functions declare parameters `value: T`, `value: &T`, and `value: &mut T`
- **THEN** calls are checked respectively as ownership transfer, shared borrow, and exclusive borrow

#### Scenario: Owned return moves automatically
- **WHEN** a function returning `T` returns a non-Copy owned local
- **THEN** ownership transfers to the caller without writing `return move value`

#### Scenario: Borrowed return is inferred
- **WHEN** a function returning `&T` returns a borrow derived from an input
- **THEN** the result's input provenance is inferred without lifetime syntax

### Requirement: Direct calls enforce ownership at arguments
After ordinary arity and type compatibility, a direct call SHALL enforce the
selected parameter's ownership mode. Shared parameters SHALL auto-borrow owned
arguments, mutable parameters SHALL require `&mut`, and owned non-Copy bound
arguments SHALL require `move`. Temporary owned expressions MAY enter owned
parameters without a move modifier.

#### Scenario: Shared parameter is concise
- **WHEN** an owned local is passed plainly to `value: &T`
- **THEN** the call borrows it and leaves the local usable

#### Scenario: Bound owned argument is explicit
- **WHEN** a non-Copy local is passed as `move value` to `value: T`
- **THEN** the call consumes the local

#### Scenario: Temporary enters owned parameter
- **WHEN** a freshly constructed value is passed to `value: T`
- **THEN** the call owns the temporary without an extra `move` marker
