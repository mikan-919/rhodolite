## Purpose

Fixes, as an explicit and tested contract, the boundary between generic
instantiation (MAP-020, MAP-025) and ownership checking: ownership checking
only ever observes fully concrete HIR with no unresolved type parameter, and
classifies, moves, borrows, clones, and drops each instantiation's values
using that instantiation's own concrete type arguments.

## Requirements

### Requirement: Ownership checking only observes fully concrete HIR
Every `hir::Callable` produced by generic instantiation, and every value
`hir::Program` handed to ownership checking, SHALL contain no unresolved
type-parameter reference anywhere in a signature, local binding, or
expression type. Ownership checking SHALL require no case, branch, or
special handling for "this callable came from a generic declaration" — an
instantiation SHALL be indistinguishable from a callable written without
type parameters.

#### Scenario: A rigid-check placeholder never reaches a checked program
- **WHEN** a program declares a generic free function and a generic impl
  method, each called at two or more distinct concrete type arguments, and
  compilation succeeds
- **THEN** the checked HIR and its ownership plan contain no rigid-check
  placeholder spelling (the `#`-prefixed synthetic name generic
  body-checking uses to keep a type parameter opaque, e.g. `#T0`/`#U1`)
  anywhere in a dumped form

#### Scenario: An incomplete type-argument substitution is rejected before ownership checking
- **WHEN** a generic instantiation's substitution does not cover every one
  of its declaration's type parameters
- **THEN** compilation fails before ownership checking runs, with a
  diagnostic pointing at the unresolved reference, and no partially
  substituted callable reaches ownership checking

### Requirement: Copy/owned classification follows an instantiation's own concrete type arguments
Ownership checking SHALL classify a generic instantiation's parameters,
locals, and return value as `Copy` or owned using that instantiation's own
substituted concrete types, independent of any other instantiation of the
same generic declaration.

#### Scenario: The same generic declaration is Copy at one type argument and owned at another
- **WHEN** a generic function `fn identity<T>(x: T -> T) { x }` is called
  once with a `Copy` argument (such as `int`) and once with a non-`Copy`
  argument (such as a struct), in the same program
- **THEN** the `int` instantiation's parameter is classified `Copy` and the
  struct instantiation's parameter is classified owned, each independent of
  the other instantiation

#### Scenario: A non-Copy value used twice without an explicit move is rejected
- **WHEN** a generic function or generic impl method instantiated at a
  non-`Copy` type argument receives a value that is then used a second time
  without `move` or `clone()`
- **THEN** compilation fails with the same move-after-use diagnostic a
  non-generic function would produce for the same shape

### Requirement: A consuming callback moves a non-Copy argument exactly once through a generic instantiation
Where a generic instantiation's body passes a non-`Copy` value into a
consuming callback parameter (a callable-typed parameter without a
reference), ownership checking SHALL plan exactly one move of that value
into the callback, for both a generic free function and a generic impl
method, using the same rule already applied to a non-generic call.

#### Scenario: `apply<T, U>` moves a non-Copy argument into its callback once
- **WHEN** the source declares `fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(move x) }`
  and calls it with a non-`Copy` argument
- **THEN** ownership checking accepts the call and the ownership plan shows
  exactly one move of the argument, at the call site and inside the
  instantiated body

#### Scenario: A generic impl method consumes a non-Copy receiver or argument once
- **WHEN** a generic impl method takes `self` or a parameter by value at a
  non-`Copy` type argument and consumes it exactly once in its body
- **THEN** ownership checking accepts the call and plans exactly one move,
  and calling the same method a second time on the same already-moved
  binding without `move`/`clone()` is rejected

### Requirement: Drop planning for a generic instantiation matches its own concrete types
Ownership checking SHALL plan a deterministic drop for an owned local or
parameter inside a generic instantiation's body according to that
instantiation's own concrete types, exactly as it would for a non-generic
body with the same substituted types.

#### Scenario: An unused owned value inside a generic instantiation is dropped
- **WHEN** a generic function instantiated at a non-`Copy` type argument
  binds a value it does not return and does not otherwise consume
- **THEN** the ownership plan includes a drop for that binding at the end of
  its scope, matching what a non-generic function with the same substituted
  type would produce
