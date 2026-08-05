## Purpose

Makes a generic free function's body provably well-typed on its own with its
type parameters held rigid, and lets a concrete call site infer type
arguments and produce a cached, fully concrete, directly executable
instantiation, without yet resolving generic trait/impl members or widening
the specialization key beyond declaration and type arguments.

## Requirements

### Requirement: A generic free function's body is checked once, independent of any call
The system SHALL type-check the body of every generic free function exactly
once, treating each of its declared type parameters as a distinct, opaque
type that is equal only to itself and supports no struct literal, field
access, method call, or operator that is not already valid for an arbitrary
unknown type. This check SHALL run whether or not the function is ever
called, and SHALL NOT lower the body into the concrete HIR that ownership
checking, requirement analysis, the interpreter, or Wasm generation consume.

#### Scenario: A well-formed generic body is accepted without being called
- **WHEN** the source declares `fn identity<T>(x: T -> T) { x }` and no call
  to `identity` exists anywhere in the program
- **THEN** compilation of the rest of the program succeeds and `identity`'s
  body is reported as well-typed

#### Scenario: A type error in an uncalled generic body is still reported
- **WHEN** the source declares a generic function whose body is only valid
  for some concrete type, such as `fn bad<T>(x: T -> int) { x + 1 }`, and the
  function is never called
- **THEN** compilation fails with a diagnostic at the ill-typed expression,
  identical in kind to the diagnostic a non-generic function with the same
  mistake would receive

#### Scenario: Two type parameters are not interchangeable
- **WHEN** the source declares `fn same<T, U>(a: T, b: U -> T) { b }`
- **THEN** compilation fails with a diagnostic reporting that the returned
  value's type does not match the declared return type, because `T` and `U`
  are held rigid and distinct even though both are unconstrained

### Requirement: Call-site type arguments are inferred from argument types
At a call to a generic free function, the system SHALL infer every type
argument by structurally matching each call argument's checked type against
the callee's corresponding generic parameter type, requiring every
occurrence of the same type parameter across the parameter list to imply the
same concrete type. The system SHALL NOT accept explicit type-argument syntax
at a call site.

#### Scenario: A single type parameter is inferred from one argument
- **WHEN** the source declares `fn identity<T>(x: T -> T) { x }` and calls
  `identity(5)`
- **THEN** compilation succeeds with `T` inferred as `int` for that call

#### Scenario: Two type parameters are inferred from two arguments
- **WHEN** the source declares
  `fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }`, a function
  `double(value: int -> int)`, and calls `apply(double, 3)`
- **THEN** compilation succeeds with `T` inferred as `int` and `U` inferred
  as `int` for that call

#### Scenario: The same call infers different type arguments at different call sites
- **WHEN** the source declares `fn identity<T>(x: T -> T) { x }` and calls it
  once as `identity(5)` and once as `identity("hi")` in the same program
- **THEN** both calls succeed, the first with `T` inferred as `int` and the
  second with `T` inferred as `str`

### Requirement: Unresolved and conflicting type arguments are diagnosed before execution
If a call to a generic free function leaves a declared type parameter with no
argument that constrains it, or if two arguments imply different concrete
types for the same type parameter, the system SHALL reject the call with a
source-positioned diagnostic before any execution, naming the type parameter
and the conflicting or missing evidence.

#### Scenario: A type parameter with no constraining argument is unresolved
- **WHEN** the source declares `fn make<T>(-> T) { ... }` (no parameter
  mentions `T`) and calls `make()`
- **THEN** compilation fails with a diagnostic reporting that the type
  argument for `T` could not be inferred

#### Scenario: Two arguments imply conflicting concrete types
- **WHEN** the source declares `fn pair<T>(a: T, b: T -> T) { a }` and calls
  `pair(1, "x")`
- **THEN** compilation fails with a diagnostic reporting that `T` was
  inferred as `int` from one argument and `str` from another

#### Scenario: Explicit type arguments are not accepted
- **WHEN** the source contains a call written with explicit type arguments
  at the call site (any syntax attempting to spell a type argument list on a
  call expression)
- **THEN** compilation fails before type argument inference runs, because no
  call-site type-argument syntax is recognized

### Requirement: A solved call site produces a cached, concrete instantiation
Once a call's type arguments are fully solved, the system SHALL build a
concrete instantiation of the callee by substituting the solved type
arguments through its signature and body, producing an ordinary, fully
concrete callable with no unbound type parameter anywhere in its signature or
body. Two calls to the same generic declaration with the same solved type
arguments SHALL share one instantiation; two calls with different type
arguments SHALL produce distinct instantiations. A generic declaration that
is never called SHALL produce no instantiation.

#### Scenario: Repeated calls with the same type arguments share one instantiation
- **WHEN** the source calls `identity(1)` twice in the same program
- **THEN** both calls resolve to the same concrete instantiation of
  `identity` for `T = int`

#### Scenario: Calls with different type arguments produce distinct instantiations
- **WHEN** the source calls `identity(1)` and `identity("a")` in the same
  program
- **THEN** two distinct concrete instantiations of `identity` exist, one for
  `T = int` and one for `T = str`, each with no unbound type parameter

### Requirement: A concrete instantiation executes through the ordinary pipeline
A concrete instantiation SHALL be an ordinary, directly-called function
indistinguishable, to ownership checking, requirement analysis, the
interpreter, and Wasm generation, from a function written without type
parameters. These passes SHALL require no new case or code path to process
a program containing generic function calls.

#### Scenario: A generic function call runs to completion
- **WHEN** a program's entry point calls `identity(5)` and returns its result
- **THEN** the interpreter runs the program and produces `5`, and Wasm
  generation for the same program succeeds and produces a byte-identical
  build across repeated builds of the same source

#### Scenario: A consuming callback still consumes its argument once
- **WHEN** the source declares
  `fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }` and calls it with a
  non-`Copy` argument
- **THEN** ownership checking accepts the call, planning exactly one move of
  the argument into `f`, using the same rule it already applies to any
  non-generic call passing a non-`Copy` value to a function parameter

### Requirement: Self-recursive instantiation with the same type arguments does not loop
If building a generic function's instantiation requires, directly or
indirectly, calling the same generic declaration again with the same solved
type arguments, the system SHALL reuse the instantiation already being built
rather than starting to build it again.

#### Scenario: A recursive generic call with the same type argument compiles
- **WHEN** the source declares a generic function that calls itself with an
  argument of the same inferred type on every recursive call, such as
  `fn count<T>(x: T, n: int -> int) { if n == 0 { 0 } else { count(x, n - 1) } }`,
  and it is called with a concrete argument
- **THEN** compilation terminates and succeeds, producing exactly one
  instantiation for that type argument

### Requirement: Polymorphic recursion is diagnosed before instantiation proceeds
If building a generic function's instantiation requires, through its own
body, instantiating the same generic declaration again with different type
arguments before the first instantiation finishes, the system SHALL reject
the program with a diagnostic identifying the generic declaration and the
differing type arguments, before attempting to build the unbounded chain of
instantiations that would otherwise result. This requirement covers a cycle
through a single generic declaration; a cycle spanning multiple distinct
generic declarations calling each other is out of scope for this capability.

#### Scenario: A generic function recursing into itself with a different type argument is rejected
- **WHEN** the source declares a generic function that, on some recursive
  path, calls itself with an argument whose type differs from the type
  argument it was itself instantiated with, such as a generic function that
  wraps its argument on each recursive call
- **THEN** compilation fails with a diagnostic reporting polymorphic
  recursion on that generic declaration, before any attempt to build further
  instantiations

## Non-Goals

- Type-checking or instantiating generic trait methods or generic impl
  methods, or any generic declaration reachable only through `trait`/`impl`
  (MAP-025).
- Including callback identity or trait implementation choice in the
  specialization key, sharing instantiations across the whole program beyond
  a single check-and-lower run, or only generating reachable instantiations
  as a deliberate reachability pass (MAP-040 widens the key; reachability
  already falls out of the existing ambient production planning that walks
  from `main` and exported roots).
- Inferring ambient/effect requirements through a generic function's
  callback parameters on a per-callback basis (MAP-050).
- Diagnosing a polymorphic-recursion cycle that spans more than one distinct
  generic declaration.
