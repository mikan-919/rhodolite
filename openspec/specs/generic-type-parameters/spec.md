## Purpose

Lets `fn`, `trait`, and `impl` declarations introduce type parameters with a
fixed declaration-scoped syntax, and gives the type checker a representation
that can hold an unsubstituted generic signature without ever letting a type
parameter reach the concrete HIR that ownership checking, the interpreter, and
Wasm generation consume.

## Requirements

### Requirement: Type parameter declaration syntax
The system SHALL accept a type parameter list `<T, U>` immediately after the
declared name of a `fn` (free function, trait method, or impl method), a
`trait`, or the `impl` keyword, written as one or more comma-separated
identifiers. The system SHALL NOT accept a type parameter list on `struct` or
`enum` declarations. An `impl` type parameter list SHALL be usable in the
`impl`'s trait reference type arguments and in its target type, including an
array target such as `[T]`.

#### Scenario: Generic function signature parses
- **WHEN** the source declares `fn identity<T>(x: T -> T) { x }`
- **THEN** parsing succeeds and the declaration records one type parameter `T`
  in scope for its parameter and return types

#### Scenario: Generic trait and array impl parse
- **WHEN** the source declares `trait Map<T> { fn map<U>(f: fn(T -> U) -> [U]) }`
  followed by `impl<T> Map<T> for [T] { fn map<U>(f: fn(T -> U) -> [U]) { ... } }`
- **THEN** parsing succeeds, the `impl`'s type parameter `T` is visible in its
  trait reference `Map<T>` and its array target `[T]`, and the method's own
  type parameter `U` is visible in the method signature

#### Scenario: Generic struct and enum declarations are rejected
- **WHEN** the source declares `struct Box<T> { value: T }` or
  `enum Option<T> { Some(T) None }`
- **THEN** parsing fails at the type parameter list with a source-positioned
  diagnostic naming the declaration kind

### Requirement: Type parameter names are scoped to their declaration
Each type parameter name SHALL resolve to a stable identifier that is valid
only within the signature, trait reference, and target type of the
declaration that introduces it (plus, for an `impl`, the signatures of its own
methods, layered on top of the `impl`'s type parameters). A name used in a
type position outside of any declaration that introduced it as a type
parameter SHALL be treated as an ordinary type name and rejected exactly as an
undeclared type is rejected today.

#### Scenario: Duplicate type parameter names are rejected
- **WHEN** a declaration writes `fn pair<T, T>(a: T, b: T -> T)` or
  `impl<T, T> Name<T> for Type { ... }`
- **THEN** compilation fails with a source-positioned diagnostic reporting the
  duplicate type parameter name before type checking proceeds

#### Scenario: A type parameter name is not visible outside its declaration
- **WHEN** one declaration introduces type parameter `T` and a separate,
  non-generic declaration uses the bare name `T` in a type annotation without
  declaring `T` itself
- **THEN** compilation fails with the same "undeclared type" diagnostic used
  for any other unknown type name, at the offending annotation's span

#### Scenario: An impl method's own type parameter shadows nothing it does not declare
- **WHEN** `impl<T> Map<T> for [T]` declares a method `fn map<U>(...)`
- **THEN** the method signature has both `T` (from the `impl`) and `U` (from
  the method) in scope, and neither name may be redeclared by the other in a
  way that is treated as a duplicate

### Requirement: Non-generic declarations are unaffected
Parsing, module resolution, type checking, HIR lowering, and every existing
diagnostic and dump for a `fn`, `trait`, or `impl` declaration with no type
parameter list SHALL be unchanged by the presence of this feature.

#### Scenario: Existing non-generic programs are unaffected
- **WHEN** a program contains no type parameter list on any declaration
- **THEN** its parsed AST, type-checked HIR, diagnostics, interpreter results,
  and generated Wasm are identical to before this change

### Requirement: Generic declarations are held as an unresolved signature, not lowered to concrete HIR
A `fn`, trait method, or impl method that declares type parameters SHALL have
its type parameter list and signature (parameter types, return type, and for
an `impl`, its trait reference and target type) validated for duplicate and
out-of-scope names and recorded in a signature representation capable of
referring to its own type parameters. This representation SHALL be kept
separate from the concrete `hir` type used by ownership checking, requirement
analysis, the interpreter, and Wasm generation; a generic declaration SHALL
NOT be registered as a callable, method, or trait implementation reachable by
those passes.

#### Scenario: A generic declaration does not reach execution
- **WHEN** a program declares a generic `fn`, trait, or `impl` and otherwise
  compiles successfully
- **THEN** ownership checking, requirement analysis, the interpreter, and Wasm
  generation run over the program's non-generic declarations only, and no
  generic declaration appears as a callable, method, or trait impl in their
  input

#### Scenario: Concrete HIR types cannot represent a type parameter
- **WHEN** any concrete `hir` type is inspected, whether produced from a
  non-generic declaration or from any future substitution of a generic
  signature's type parameters
- **THEN** it is one of the existing concrete forms (builtin, struct, enum,
  array, callable, or the poison marker) and never an unbound type parameter,
  which is checkable by a dedicated invariant helper and covered by a unit
  test
