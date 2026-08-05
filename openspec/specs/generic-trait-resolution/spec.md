## Purpose

Checks a generic `impl`'s methods against its trait's declared contract with
type parameters substituted and corresponded, and lets a method call on a
concrete receiver resolve, uniquely, into a cached, concrete, directly
executable generic `impl` method instance — without yet resolving an impl
whose target is not a declared struct or widening the specialization key
beyond declaration and type arguments.

## Requirements

### Requirement: A generic impl's trait reference is validated
The system SHALL reject a generic `impl`'s trait reference whose name does
not resolve to a declared trait, and SHALL reject a trait reference whose
type-argument count does not match that trait's own declared number of type
parameters, each with a source-positioned diagnostic naming the trait and,
for an arity mismatch, both the expected and given counts.

#### Scenario: An impl's trait reference names an undeclared trait
- **WHEN** the source declares `impl<T> Missing<T> for Box { ... }` and no
  trait named `Missing` is declared anywhere in the program
- **THEN** compilation fails with a diagnostic reporting that `Missing` is
  not a trait

#### Scenario: An impl's trait reference has the wrong number of type arguments
- **WHEN** the source declares `trait Pair<T, U> { ... }` and
  `impl<T> Pair<T> for Box { ... }` (one type argument where the trait
  declares two)
- **THEN** compilation fails with a diagnostic reporting the expected and
  given type-argument counts

### Requirement: A generic impl's target must name a declared struct
The system SHALL require a generic `impl`'s target type, after substituting
the impl's own type parameters, to name a declared struct, using the same
requirement and diagnostic a non-generic `impl`'s target already has.

#### Scenario: A generic impl targeting an array is rejected
- **WHEN** the source declares `impl<T> Foo<T> for [T] { ... }`
- **THEN** compilation fails with a diagnostic reporting that the target is
  not a struct

### Requirement: A generic impl's methods are contract-checked against the trait's declared signature
The system SHALL check that a generic `impl`'s provided methods exactly match
the set the referenced trait declares, and that each provided method's
receiver, parameter types, and return type match the trait's declared
signature for that method once the trait's own type parameters are
substituted through the impl's trait-reference arguments and the trait
method's own type parameters are corresponded to the impl method's own type
parameters by declaration position. This check SHALL run whether or not any
call site ever resolves to the impl.

#### Scenario: A matching generic impl is accepted
- **WHEN** the source declares `trait Box<T> { fn wrap<U>(self, f: fn(T -> U) -> U) }`
  and `impl<T> Box<T> for Container { fn wrap<U>(self, f: fn(T -> U) -> U) { f(self.value) } }`
- **THEN** compilation of the impl's contract succeeds

#### Scenario: An impl omitting a trait-declared method is rejected
- **WHEN** the source declares a trait with two methods and a generic impl
  of that trait providing only one of them
- **THEN** compilation fails with a diagnostic naming the trait, the type,
  and the omitted method

#### Scenario: An impl implementing the same method twice is rejected
- **WHEN** a generic impl block declares two methods with the same name
- **THEN** compilation fails with a diagnostic reporting that the method is
  implemented twice

#### Scenario: A provided method's parameter type does not match the trait's contract
- **WHEN** the source declares `trait Box<T> { fn wrap<U>(self, f: fn(T -> U) -> U) }`
  and an impl of it whose `wrap` takes `f: fn(int -> U)` regardless of `T`
- **THEN** compilation fails with a diagnostic reporting the mismatched
  parameter type, naming both the trait's declared type and the impl's
  provided type

#### Scenario: A provided method's own type parameter count does not match the trait's
- **WHEN** the source declares `trait Box<T> { fn wrap<U>(self, f: fn(T -> U) -> U) }`
  and an impl whose `wrap` declares no type parameters of its own
- **THEN** compilation fails with a diagnostic reporting the mismatched
  signature

### Requirement: Two impls of the same trait for the same type are rejected
If a generic or non-generic `impl` of a trait for a struct is declared, and a
second `impl` (generic or non-generic) of the same trait for the same struct
is also declared, the system SHALL reject the second with a source-positioned
diagnostic naming the trait, the struct, and both declarations' locations.

#### Scenario: Two generic impls of the same trait for the same type are rejected
- **WHEN** the source declares `impl<T> Box<T> for Container { ... }` twice
  for the same trait `Box` and the same struct `Container`
- **THEN** compilation fails with a diagnostic naming both `impl` locations

### Requirement: A generic impl method's body is checked once, independent of any call
The system SHALL type-check the body of every generic impl method exactly
once, treating each of its own type parameters and each of its enclosing
impl's type parameters as a distinct, opaque type equal only to itself. This
check SHALL run whether or not any call site ever resolves to the method, and
SHALL NOT lower the body into the concrete HIR that ownership checking,
requirement analysis, the interpreter, or Wasm generation consume.

#### Scenario: A well-formed generic impl method is accepted without being called
- **WHEN** the source declares a generic impl method whose body is valid for
  an arbitrary type parameter, and no call resolves to it
- **THEN** compilation succeeds and the method's body is reported as
  well-typed

#### Scenario: A type error in an uncalled generic impl method body is still reported
- **WHEN** the source declares a generic impl method whose body is only
  valid for some concrete type, and no call resolves to it
- **THEN** compilation fails with a diagnostic at the ill-typed expression

### Requirement: Method-call resolution falls back to generic impls only when existing resolution finds no match
At a method-call expression whose receiver has a concrete, named type, the
system SHALL first attempt today's existing (non-generic) method resolution
unchanged. Only when that resolution finds no match SHALL the system search
declared generic impls whose target names the receiver's type for a method
with the called name, and, from a unique match, infer every remaining type
argument by structurally matching each call argument's checked type against
the method's corresponding parameter type, requiring every occurrence of the
same type parameter to imply the same concrete type. The system SHALL NOT
change the outcome of a method call that today's existing resolution already
resolves.

#### Scenario: A generic impl method call resolves and infers its type argument
- **WHEN** the source declares `trait Box<T> { fn wrap<U>(self, f: fn(T -> U) -> U) }`,
  a matching `impl<T> Box<T> for Container`, a `Container` value `c`, and a
  function `double(value: int -> int)`, and calls `c.wrap(double)`
- **THEN** compilation succeeds with the impl's own type parameter and the
  method's own type parameter both inferred from the call

#### Scenario: An existing non-generic method resolution is unaffected
- **WHEN** the source declares a non-generic `impl Trait for Type` providing
  method `m`, and a value of `Type` calls `m`
- **THEN** compilation resolves the call exactly as it did before this
  capability existed, through the existing non-generic resolution path

### Requirement: A method call with no matching generic impl produces the existing diagnostic
If no non-generic method matches and no generic impl's target names the
receiver's type with the called method name, the system SHALL produce the
same diagnostic a method call with no matching declaration produces today.

#### Scenario: A method call with no implementation at all is unaffected
- **WHEN** the source calls a method name no trait, inherent impl, or generic
  impl of the receiver's type declares
- **THEN** compilation fails with the same "call target cannot be determined"
  diagnostic this would have produced without this capability

### Requirement: A method call matching more than one generic impl is diagnosed as ambiguous
If more than one generic impl's target names the receiver's type with the
called method name, the system SHALL reject the call with a source-positioned
diagnostic naming the receiver's type, the method name, and each competing
impl's trait and declaration location, before attempting to infer any type
argument.

#### Scenario: Two generic impls providing the same method name are ambiguous
- **WHEN** the source declares two different traits, each with a method named
  `wrap`, both implemented generically for the same struct `Container`, and
  calls `container.wrap(x)`
- **THEN** compilation fails with a diagnostic reporting the ambiguity and
  naming both candidate traits

### Requirement: Unresolved and conflicting type arguments at a generic impl method call are diagnosed before execution
If a generic impl method call leaves one of its type parameters with no
argument that constrains it, or if two arguments imply different concrete
types for the same type parameter, the system SHALL reject the call with a
source-positioned diagnostic before any execution, naming the type parameter
and the conflicting or missing evidence — the same diagnostic shape a generic
free function call already produces for the same failure.

#### Scenario: A generic impl method's type parameter has no constraining argument
- **WHEN** a generic impl method's own type parameter is not mentioned by any
  of its parameters, and a call to it is made
- **THEN** compilation fails with a diagnostic reporting that the type
  argument could not be inferred

#### Scenario: Two arguments to a generic impl method imply conflicting types
- **WHEN** a generic impl method's signature uses the same type parameter for
  two parameters, and a call passes arguments of two different concrete types
- **THEN** compilation fails with a diagnostic naming both inferred types

### Requirement: A resolved generic impl method call produces a cached, concrete instantiation
Once a generic impl method call's type arguments are fully solved, the system
SHALL build a concrete instantiation of the method by substituting the solved
type arguments through its signature and body, producing an ordinary, fully
concrete, directly executable callable with no unbound type parameter
anywhere in its signature or body. Two calls to the same generic impl method
with the same solved type arguments SHALL share one instantiation; two calls
with different type arguments SHALL produce distinct instantiations. This
instantiation SHALL be indistinguishable, to ownership checking, requirement
analysis, the interpreter, and Wasm generation, from a method written without
type parameters.

#### Scenario: Repeated calls with the same type arguments share one instantiation
- **WHEN** the source calls the same generic impl method twice with arguments
  of the same concrete type
- **THEN** both calls resolve to the same concrete instantiation

#### Scenario: Calls with different type arguments produce distinct instantiations
- **WHEN** the source calls the same generic impl method once with an `int`
  argument and once with a `str` argument
- **THEN** two distinct concrete instantiations exist, each with no unbound
  type parameter

### Requirement: Polymorphic recursion through a generic impl method is diagnosed before instantiation proceeds
If building a generic impl method's instantiation requires, through its own
body, instantiating the same generic impl method again with different type
arguments before the first instantiation finishes, the system SHALL reject
the program with a diagnostic identifying the method and the differing type
arguments, before attempting to build the unbounded chain of instantiations
that would otherwise result.

#### Scenario: A generic impl method recursing into itself with a different type argument is rejected
- **WHEN** a generic impl method's body, on some recursive path, calls itself
  with an argument whose type differs from the type argument it was itself
  instantiated with
- **THEN** compilation fails with a diagnostic reporting polymorphic
  recursion, before any attempt to build further instantiations

## Non-Goals

- Resolving or contract-checking an impl whose target does not name a
  declared struct (an array, callable, builtin, or blanket impl target).
- Resolving a generic trait method through ambient/slot dispatch (`with` /
  a slot-typed receiver).
- Including callback identity in the specialization key, or generating only
  reachable instantiations as a deliberate whole-program pass (MAP-040).
- Feeding a resolved instantiation's concrete types through any new
  ownership-specific rule or dedicated ownership test matrix (MAP-030).
- Diagnosing a polymorphic-recursion cycle that spans more than one distinct
  generic declaration, or one that alternates between a generic free
  function and a generic impl method.
