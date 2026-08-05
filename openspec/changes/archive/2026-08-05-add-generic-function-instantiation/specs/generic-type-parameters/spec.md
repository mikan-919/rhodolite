## MODIFIED Requirements

### Requirement: Generic declarations are held as an unresolved signature, not lowered to concrete HIR
A `fn`, trait method, or impl method that declares type parameters SHALL have
its type parameter list and signature (parameter types, return type, and for
an `impl`, its trait reference and target type) validated for duplicate and
out-of-scope names and recorded in a signature representation capable of
referring to its own type parameters. This representation SHALL be kept
separate from the concrete `hir` type used by ownership checking, requirement
analysis, the interpreter, and Wasm generation. A generic trait method or
generic impl method SHALL NOT be registered as a callable, method, or trait
implementation reachable by those passes. A generic free function SHALL NOT
be registered as a callable by those passes either, except that a call to it
from concrete, non-generic code produces a separate, fully concrete
instantiation (see the `generic-function-instantiation` capability), which is
itself an ordinary callable those passes do process.

#### Scenario: An uncalled generic declaration does not reach execution
- **WHEN** a program declares a generic `fn`, `trait`, or `impl` that no
  concrete call site ever calls, and the program otherwise compiles
  successfully
- **THEN** ownership checking, requirement analysis, the interpreter, and
  Wasm generation run without observing that declaration as a callable,
  method, or trait impl in their input

#### Scenario: A generic trait or impl declaration never reaches execution directly
- **WHEN** a program declares a generic `trait` or `impl` and otherwise
  compiles successfully
- **THEN** ownership checking, requirement analysis, the interpreter, and
  Wasm generation never observe that trait or impl declaration, or any of its
  methods, as a callable, method, or trait impl in their input, regardless of
  whether any call site exists

#### Scenario: A called generic free function reaches execution through its instantiation
- **WHEN** a program declares a generic free function and a concrete call
  site calls it with arguments that let every type argument be inferred
- **THEN** ownership checking, requirement analysis, the interpreter, and
  Wasm generation observe a concrete, fully-typed callable produced from that
  call, but never observe the generic declaration itself as a callable

#### Scenario: Concrete HIR types cannot represent a type parameter
- **WHEN** any concrete `hir` type is inspected, whether produced from a
  non-generic declaration or from substituting a generic function's type
  parameters at a call site
- **THEN** it is one of the existing concrete forms (builtin, struct, enum,
  array, callable, or the poison marker) and never an unbound type parameter,
  which is checkable by a dedicated invariant helper and covered by a unit
  test
