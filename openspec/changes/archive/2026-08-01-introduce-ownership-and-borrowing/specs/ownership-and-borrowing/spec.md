## Purpose

Defines single ownership and statically checked borrowing so Rhodolite can be
memory-safe without garbage collection, runtime borrow checks, or written
lifetime parameters while keeping ordinary read-only calls concise.

## ADDED Requirements

### Requirement: Values have explicit ownership modes
Every non-Copy value SHALL have exactly one owner and MAY be accessed through
shared `&T` or exclusive `&mut T` borrows. Source SHALL express reference types
as `&T` and `&mut T` without lifetime parameters. An owned type `T`, `&T`, and
`&mut T` SHALL be distinct static types.

#### Scenario: Shared borrow type
- **WHEN** a signature declares a parameter `user: &User`
- **THEN** the function may read the same owned `User` without taking or copying it

#### Scenario: Mutable borrow type
- **WHEN** a signature declares a parameter `user: &mut User`
- **THEN** the function receives exclusive mutable access without taking ownership

#### Scenario: Lifetime spelling is rejected
- **WHEN** source attempts to write a Rust-style lifetime such as `&'a User`
- **THEN** parsing fails because lifetime parameters are not source-language syntax

### Requirement: Transfer and copying remain visible
Binding a non-Copy owned local to another local SHALL move it by default. Passing
a bound non-Copy value to an owned function parameter or consuming receiver SHALL
require the `move` modifier. Deep copying SHALL occur only through explicit
`clone()`. Copy values MAY be copied implicitly.

#### Scenario: Local binding moves
- **WHEN** source evaluates `let second = first` for a non-Copy value
- **THEN** `second` becomes the owner and any later use of `first` is rejected

#### Scenario: Owned call requires move
- **WHEN** an owned local is passed to a parameter of owned type without `move`
- **THEN** checking fails at the argument and suggests an explicit move or borrow

#### Scenario: Explicit clone preserves the owner
- **WHEN** source evaluates `let second = first.clone()` for cloneable owned data
- **THEN** both bindings own independent structurally equal values

#### Scenario: Scalar binding copies
- **WHEN** an `int`, `bool`, `unit`, or fieldless-enum value is rebound
- **THEN** the source binding remains usable without `clone()`

### Requirement: Shared reads are concise and effects are explicit
A call parameter of type `&T` SHALL borrow a compatible owned argument without a
call-site `&`. A call parameter of type `&mut T` SHALL require call-site `&mut`.
An owned parameter SHALL require call-site `move` for a bound non-Copy value.
Ownership modifiers SHALL bind to a place or receiver before its postfix field
and call chain, so parentheses are not required.

#### Scenario: Read-only call auto-borrows
- **WHEN** `inspect(user)` selects a parameter of type `&User`
- **THEN** the call creates a shared borrow and `user` remains owned by the caller

#### Scenario: Mutating call is visible
- **WHEN** `rename(&mut user, name)` selects a parameter of type `&mut User`
- **THEN** the call is accepted only when `user` is mutably accessible and exclusive

#### Scenario: Receiver modifier needs no parentheses
- **WHEN** source calls `&mut user.rename(name)` or `move user.finish()`
- **THEN** the modifier applies to `user` as the selected method receiver

### Requirement: Borrow safety is checked before execution
The checker SHALL reject use after move, move while borrowed, mutation through a
shared borrow, overlapping mutable access, and any borrow that outlives its
owner. These checks SHALL cover every loaded body and SHALL NOT rely on runtime
borrow counters, garbage collection, raw pointers, or an `unsafe` escape hatch.

#### Scenario: Use after move
- **WHEN** a continuing control-flow path uses a non-Copy local after ownership moved
- **THEN** checking fails at that later use and identifies the move source

#### Scenario: Shared and mutable borrows overlap
- **WHEN** a mutable borrow overlaps a still-live shared borrow of the same place
- **THEN** checking fails with both conflicting access locations

#### Scenario: Borrow escapes its owner
- **WHEN** a returned or assigned borrow can remain live after its owner is dropped
- **THEN** checking fails before requirement analysis or execution

### Requirement: Lifetimes and return provenance are inferred whole-program
The checker SHALL infer the last use of every borrow and the input or input-field
provenance of every borrowed return across the loaded call graph. Branches and
recursive calls SHALL converge to conservative provenance sets. A result whose
provenance has multiple possible owners SHALL remain valid only while every
candidate owner remains valid.

#### Scenario: Borrow ends at last use
- **WHEN** a shared borrow's last use precedes a later mutable access in one scope
- **THEN** the later mutable access is accepted without an explicit lifetime block

#### Scenario: Return borrows one input
- **WHEN** a function returns a field borrowed from one input
- **THEN** callers may retain the result only while that input field remains valid

#### Scenario: Return may borrow either input
- **WHEN** a function conditionally returns a borrow from either of two inputs
- **THEN** callers conservatively keep both candidate owners borrowed for the result's lifetime

### Requirement: Disjoint places may be borrowed independently
Borrow conflicts SHALL be tracked at least to distinct struct-field paths.
Different known struct fields MAY be mutably borrowed independently. A field and
its containing value SHALL overlap. Enum payloads and dynamically indexed array
elements SHALL initially be treated conservatively as overlapping their whole
container unless non-overlap is statically proven.

#### Scenario: Different struct fields
- **WHEN** code mutably borrows `pair.left` and `pair.right`
- **THEN** both borrows are accepted as disjoint

#### Scenario: Whole value conflicts with a field
- **WHEN** code borrows `pair.left` and mutably accesses `pair`
- **THEN** checking rejects the overlapping whole-value access

#### Scenario: Dynamic array indices
- **WHEN** two mutable borrows use indices whose inequality cannot be proven
- **THEN** checking conservatively rejects the second borrow

### Requirement: References are not yet aggregate-owned data
This version SHALL allow references in locals, parameters, and results but SHALL
reject storing a reference inside a struct, enum, optional, or array and SHALL
reject references in public Wasm ABI signatures. Shared ownership and raw
pointer types SHALL remain unavailable.

#### Scenario: Reference field is deferred
- **WHEN** a struct declares a field whose type is `&T` or `&mut T`
- **THEN** checking reports that borrowed aggregate fields are not supported yet

#### Scenario: Public borrowed result is rejected
- **WHEN** an entry-module public function exposes a borrowed result
- **THEN** the Wasm build rejects the public signature before emission
