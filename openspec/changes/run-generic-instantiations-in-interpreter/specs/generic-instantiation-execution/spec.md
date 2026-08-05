## Purpose

Fixes, as an explicit and tested contract, the boundary between generic
instantiation (MAP-020, MAP-025) and the HIR interpreter (`src/eval.rs`):
the interpreter runs any generic function or generic trait method
instantiation exactly as it would run a hand-written callable, and a
consuming callback's value, drop, and ambient-requirement behavior for an
instantiation matches what ownership checking and ambient-ABI planning
already planned for it.

## ADDED Requirements

### Requirement: The interpreter runs a generic instantiation like any other callable
The interpreter SHALL execute a generic function's or generic trait
method's instantiation with no case, branch, or special handling for
"this callable came from a generic declaration" — an instantiation SHALL be
indistinguishable, at every call kind the interpreter dispatches (direct,
associated, method, indirect, and slot/ambient calls), from a callable
written without type parameters.

#### Scenario: A generic function instantiation runs to completion
- **WHEN** a program's entry point calls `identity<T>(x: T -> T) { x }` and
  returns its result
- **THEN** the interpreter runs the program to completion and produces the
  argument's value unchanged

#### Scenario: A generic trait method instantiation runs to completion
- **WHEN** a program declares a generic trait, a generic impl of it for a
  concrete struct, and calls the resulting generic method through an
  ordinary method-call expression
- **THEN** the interpreter resolves the call to the method's concrete
  instantiation and runs it to completion, producing the same result a
  hand-written (non-generic) method with the same body would produce

### Requirement: The same generic body called with different type arguments produces independent results
Where a program calls the same generic declaration with two or more
distinct sets of type arguments (and/or distinct callback bindings), the
interpreter SHALL run each resulting instantiation independently, using
that instantiation's own arguments and its own concrete types, with no
value, binding, or ambient state leaking from one instantiation's
execution into another's.

#### Scenario: The same generic function is called at two different type arguments
- **WHEN** a program calls `identity(5)` and `identity("ok")` in the same
  entry point and combines both results
- **THEN** the interpreter runs both calls and each produces the value
  passed to it at its own type argument, independent of the other call

#### Scenario: The same generic declaration is called with two different callback bindings
- **WHEN** a program declares `fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }`
  and two named functions, and calls `apply` once bound to each
- **THEN** the interpreter runs each call through its own instantiation and
  each produces the result of applying its own bound callback, independent
  of the other call

### Requirement: A consuming callback's value, drop, and ambient behavior matches the checked plan
Where a generic instantiation's body passes a value into a consuming
callback parameter, the interpreter SHALL move that value into the
callback exactly once, matching the move ownership checking already
planned for it, and SHALL drop any owned value the instantiation does not
return exactly where the ownership plan places that drop. Where the
selected callback requires an ambient value, the interpreter SHALL resolve
that requirement from the ambient bindings in scope at the call site
exactly as it does for a non-generic callback, and a sibling instantiation
of the same generic declaration whose callback requires no ambient value
SHALL run without consulting any ambient binding.

#### Scenario: A non-Copy argument is moved into a consuming callback exactly once
- **WHEN** the source declares `fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(move x) }`
  and calls it with a non-`Copy` argument
- **THEN** the interpreter runs the call, the callback observes the moved
  value exactly once, and no allocation the program made remains live after
  the program finishes running

#### Scenario: An ambient-requiring callback resolves its requirement through a generic call
- **WHEN** a program declares a generic `apply<T, U>` called with a callback
  that reads an ambient value, invoked inside a `with` block that provides
  that value
- **THEN** the interpreter runs the call and the callback observes the
  value provided by the enclosing `with`

#### Scenario: A callback-free sibling instantiation needs no ambient binding
- **WHEN** the same generic declaration as the previous scenario is also
  called, in the same program, bound to a callback that reads no ambient
  value, outside of any `with` block
- **THEN** the interpreter runs that call to completion without requiring
  any ambient value to be in scope
