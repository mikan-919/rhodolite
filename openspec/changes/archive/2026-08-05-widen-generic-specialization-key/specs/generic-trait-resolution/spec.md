## MODIFIED Requirements

### Requirement: A resolved generic impl method call produces a cached, concrete instantiation
Once a generic impl method call's type arguments are fully solved, the system
SHALL build a concrete instantiation of the method by substituting the solved
type arguments through its signature and body, producing an ordinary, fully
concrete, directly executable callable with no unbound type parameter
anywhere in its signature or body. As with a generic free function
(`generic-function-instantiation`), the instantiation SHALL be cached by the
generic impl method's own declaration identity, its solved type arguments,
and its callback binding. Two calls to the same generic impl method with the
same solved type arguments and the same callback binding SHALL share one
instantiation; two calls that differ in either their type arguments or their
callback binding SHALL produce distinct instantiations. This instantiation
SHALL be indistinguishable, to ownership checking, requirement analysis, the
interpreter, and Wasm generation, from a method written without type
parameters.

#### Scenario: Repeated calls with the same type arguments share one instantiation
- **WHEN** the source calls the same generic impl method twice with arguments
  of the same concrete type
- **THEN** both calls resolve to the same concrete instantiation

#### Scenario: Calls with different type arguments produce distinct instantiations
- **WHEN** the source calls the same generic impl method once with an `int`
  argument and once with a `str` argument
- **THEN** two distinct concrete instantiations exist, each with no unbound
  type parameter

#### Scenario: Calls with the same type arguments but different callback bindings produce distinct instantiations
- **WHEN** the source declares a generic impl method taking a callable-typed
  parameter, and two calls to that method solve the same type arguments but
  pass different concrete functions for that parameter
- **THEN** two distinct concrete instantiations of the method exist, one per
  callback binding

### Requirement: Polymorphic recursion through a generic impl method is diagnosed before instantiation proceeds
If building a generic impl method's instantiation requires, through its own
body, instantiating the same generic impl method again with different type
arguments before the first instantiation finishes, the system SHALL reject
the program with a diagnostic identifying the method and the differing type
arguments, before attempting to build the unbounded chain of instantiations
that would otherwise result. As with a generic free function, this considers
only a change in type arguments; a recursive call whose callback binding
differs while its type arguments stay the same is not polymorphic recursion.

#### Scenario: A generic impl method recursing into itself with a different type argument is rejected
- **WHEN** a generic impl method's body, on some recursive path, calls itself
  with an argument whose type differs from the type argument it was itself
  instantiated with
- **THEN** compilation fails with a diagnostic reporting polymorphic
  recursion, before any attempt to build further instantiations
