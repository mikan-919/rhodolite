## MODIFIED Requirements

### Requirement: A solved call site produces a cached, concrete instantiation
Once a call's type arguments are fully solved, the system SHALL build a
concrete instantiation of the callee by substituting the solved type
arguments through its signature and body, producing an ordinary, fully
concrete callable with no unbound type parameter anywhere in its signature or
body. The instantiation SHALL be cached by the generic declaration, its
solved type arguments, and its callback binding: for each of the callee's
declared parameters, whichever concrete named function (if any) the call
site passes for that parameter, resolved the same way requirement analysis
already resolves callback identity for non-generic calls. Two calls to the
same generic declaration with the same solved type arguments and the same
callback binding SHALL share one instantiation; two calls that differ in
either their type arguments or their callback binding SHALL produce distinct
instantiations. A generic declaration that is never called SHALL produce no
instantiation, and no instantiation is produced for a type-argument/callback
combination no call site actually uses.

#### Scenario: Repeated calls with the same type arguments share one instantiation
- **WHEN** the source calls `identity(1)` twice in the same program
- **THEN** both calls resolve to the same concrete instantiation of
  `identity` for `T = int`

#### Scenario: Calls with different type arguments produce distinct instantiations
- **WHEN** the source calls `identity(1)` and `identity("a")` in the same
  program
- **THEN** two distinct concrete instantiations of `identity` exist, one for
  `T = int` and one for `T = str`, each with no unbound type parameter

#### Scenario: Calls with the same type arguments but different callback bindings produce distinct instantiations
- **WHEN** the source declares `fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }`
  and two functions `double(value: int -> int)` and `triple(value: int -> int)`,
  and calls `apply(double, 1)` and `apply(triple, 1)`
- **THEN** both calls solve `T = int, U = int`, but two distinct concrete
  instantiations of `apply` exist — one whose callback binding is `double`,
  one whose callback binding is `triple` — and `main`'s two call expressions
  target different instantiations

#### Scenario: Calls with the same type arguments and the same callback binding share one instantiation
- **WHEN** the source declares `fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }`
  and a function `double(value: int -> int)`, and calls `apply(double, 1)`
  and `apply(double, 2)`
- **THEN** both calls solve `T = int, U = int` with the same callback binding
  (`double`), and both resolve to the same concrete instantiation of `apply`

### Requirement: Self-recursive instantiation with the same type arguments does not loop
If building a generic function's instantiation requires, directly or
indirectly, calling the same generic declaration again with the same solved
type arguments and the same callback binding, the system SHALL reuse the
instantiation already being built rather than starting to build it again.

#### Scenario: A recursive generic call with the same type argument compiles
- **WHEN** the source declares a generic function that calls itself with an
  argument of the same inferred type on every recursive call, such as
  `fn count<T>(x: T, n: int -> int) { if n == 0 { 0 } else { count(x, n - 1) } }`,
  and it is called with a concrete argument
- **THEN** compilation terminates and succeeds, producing exactly one
  instantiation for that type argument

#### Scenario: A recursive generic call forwarding its own callback argument compiles to one instantiation
- **WHEN** the source declares
  `fn apply<T>(f: fn(T -> T), x: T, n: int -> T) { if n == 0: x else: apply(f, f(x), n - 1) }`
  and calls it with a concrete argument and callback
- **THEN** compilation terminates and succeeds, producing exactly one
  instantiation, because every recursive call forwards the same callback
  binding it was itself instantiated with

### Requirement: Polymorphic recursion is diagnosed before instantiation proceeds
If building a generic function's instantiation requires, through its own
body, instantiating the same generic declaration again with different type
arguments before the first instantiation finishes, the system SHALL reject
the program with a diagnostic identifying the generic declaration and the
differing type arguments, before attempting to build the unbounded chain of
instantiations that would otherwise result. This requirement covers a cycle
through a single generic declaration; a cycle spanning multiple distinct
generic declarations calling each other is out of scope for this capability.
Polymorphic-recursion detection considers only a change in type arguments: a
recursive call whose callback binding differs from the instantiation
currently being built, while its type arguments stay the same, is not
polymorphic recursion (see the `generic-function-instantiation` capability's
Non-Goals for why a callback binding cannot itself diverge across recursion
depth in this language).

#### Scenario: A generic function recursing into itself with a different type argument is rejected
- **WHEN** the source declares a generic function that, on some recursive
  path, calls itself with an argument whose type differs from the type
  argument it was itself instantiated with, such as a generic function that
  wraps its argument on each recursive call
- **THEN** compilation fails with a diagnostic reporting polymorphic
  recursion on that generic declaration, before any attempt to build further
  instantiations
