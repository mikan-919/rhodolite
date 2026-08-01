# method-call-type-checking

## Purpose

Resolve calls that go through a value, an ambient slot, or a type path, and
check them against the declaration they resolve to. Trait implementations are
validated against their contracts at declaration time, so a resolved call has
one known signature whose arity, argument types, and return type flow into the
checks the other capabilities already perform.

## Requirements

### Requirement: Trait implementations conform to declared contracts
The checker SHALL require every trait `impl` to name a declared trait and struct and to implement the trait's complete method set exactly once. Each implementation method SHALL match the declared receiver form, parameter types in order, and optional return type; parameter names need not match. Methods not declared by the trait SHALL NOT appear in that trait `impl`.

#### Scenario: Complete matching implementation
- **WHEN** a trait implementation provides every declared method once with matching receiver, parameter, and return types
- **THEN** the checker produces no trait-conformance diagnostic

#### Scenario: Missing trait method
- **WHEN** a trait implementation omits a method declared by the trait
- **THEN** the checker reports the implementation, trait, and missing method

#### Scenario: Extra or duplicate implementation method
- **WHEN** a trait implementation contains an undeclared method or declares one method more than once
- **THEN** the checker reports the offending implementation method

#### Scenario: Mismatched implementation signature
- **WHEN** an implementation method differs from its trait declaration in receiver form, parameter types, parameter count, or return type
- **THEN** the checker reports the method and the mismatched part of its signature

#### Scenario: Unknown trait or target type
- **WHEN** a trait implementation names an undeclared trait or a target that is not a declared struct
- **THEN** the checker reports the invalid declaration

### Requirement: Calls resolve with the language's declared candidate rules
The checker SHALL resolve every call by callee form. A dot call on a concrete receiver SHALL select by concrete type and method name across inherent and trait implementations. A dot call on an ambient slot SHALL select the named slot trait's declaration. A `Type::function` call SHALL select by concrete type and function name, and a `slot::function` call SHALL select the named slot trait's declaration. Concrete-type lookup SHALL require exactly one named candidate before validating whether its receiver form matches the call syntax. Failure to determine the receiver type or a unique candidate SHALL fail checking.

#### Scenario: Concrete receiver selects a unique method
- **WHEN** a concrete receiver's type has exactly one implementation candidate with the called name
- **THEN** the checker resolves the dot call to that candidate

#### Scenario: Ambient slot selects its trait
- **WHEN** a dot or path call begins with an unshadowed ambient slot
- **THEN** the checker resolves the call only against that slot's declared trait

#### Scenario: Concrete type selects a unique associated function
- **WHEN** a declared concrete type has exactly one implementation candidate with the called path name
- **THEN** the checker resolves the path call to that candidate

#### Scenario: Unknown member
- **WHEN** a known concrete type or ambient slot has no candidate with the called name
- **THEN** the checker reports that the member does not exist

#### Scenario: Ambiguous concrete member
- **WHEN** a concrete type has more than one implementation candidate with the called name and no slot trait selects one
- **THEN** the checker reports that the member's trait cannot be determined

#### Scenario: Receiver type cannot be determined
- **WHEN** a dot-call receiver has no concrete type and is not an ambient slot
- **THEN** checking fails at the receiver instead of deferring resolution

### Requirement: Call syntax matches the declared receiver form
The checker SHALL require dot calls to resolve to a signature that declares `self` and type or slot path calls to resolve to a signature that does not declare `self`.

#### Scenario: Method called with dot syntax
- **WHEN** a resolved signature declares `self` and is called through a value or slot with dot syntax
- **THEN** the checker produces no receiver-form diagnostic

#### Scenario: Associated function called with path syntax
- **WHEN** a resolved signature does not declare `self` and is called through a type or slot path
- **THEN** the checker produces no receiver-form diagnostic

#### Scenario: Associated function called as a method
- **WHEN** a resolved signature does not declare `self` but is called with dot syntax
- **THEN** the checker reports that the function must be called through a type path

#### Scenario: Method called through a path
- **WHEN** a resolved signature declares `self` but is called through a type or slot path
- **THEN** the checker reports that the method requires a receiver value

### Requirement: Resolved calls enforce declared signatures
The checker SHALL compare a resolved call's explicit argument count and every argument type with the selected signature. Argument compatibility SHALL use the existing directional destination rule, including `T` to `T?` injection but not `T?` to `T`. Failure to determine an argument type SHALL fail checking.

#### Scenario: Matching resolved call
- **WHEN** a resolved method or associated-function call has the declared number of compatible arguments
- **THEN** the checker produces no call-signature diagnostic

#### Scenario: Wrong argument count
- **WHEN** a resolved call has fewer or more explicit arguments than its selected signature declares after excluding `self`
- **THEN** the checker reports the member and expected and actual argument counts

#### Scenario: Wrong argument type
- **WHEN** an argument is incompatible with its resolved parameter type
- **THEN** the checker reports the member, argument position, expected type, and actual type

#### Scenario: Argument type cannot be determined
- **WHEN** the checker cannot determine a resolved call argument's type
- **THEN** checking fails at that argument

### Requirement: Resolved call results carry return types
The checker SHALL assign the selected signature's effective return type to every resolved method or associated-function call and SHALL expose it to bindings and all expression, assignment, argument, and return checks. A selected signature without a return annotation SHALL have effective return type `unit`.

#### Scenario: Method result feeds optional fallback
- **WHEN** a resolved method returns `T?` and its result is the left operand of `??` with a compatible `T` fallback
- **THEN** the checker infers the fallback expression as `T`

#### Scenario: Associated function result feeds field access
- **WHEN** a resolved associated function returns a declared struct type and the result is used as a field receiver
- **THEN** the checker validates the field against that struct declaration

#### Scenario: Resolved result reaches an existing mismatch
- **WHEN** a resolved call's return type is incompatible with a destination or operand
- **THEN** the existing compatibility check reports the mismatch

#### Scenario: No return annotation
- **WHEN** a resolved member has no return annotation
- **THEN** the call result has type `unit`

### Requirement: Canonical calls are statically typed
The canonical program SHALL continue to pass checking and execution while its slot methods and associated constructors participate in static type checking.

#### Scenario: Canonical optional method result
- **WHEN** the canonical program is checked
- **THEN** `db.find(id)` has type `User?`, `?? return false` yields `User`, and subsequent user field operations are checked

#### Scenario: Canonical scalar and constructor results
- **WHEN** the canonical program is checked
- **THEN** `clock.now()` has type `int` and the `Postgres`, `InMemoryDb`, and `Frozen` associated constructors have their declared result types

#### Scenario: Canonical behavior is unchanged
- **WHEN** the canonical main program and test are executed
- **THEN** their existing successful results remain unchanged

### Requirement: Methods declare receiver ownership
Method and trait signatures SHALL distinguish shared `&self`, mutable `&mut
self`, and consuming `self`. Implementations SHALL match the trait receiver mode
exactly. Associated functions SHALL continue to omit a receiver.

#### Scenario: Shared receiver
- **WHEN** a method declares `&self`
- **THEN** an ordinary dot call shared-borrows its receiver

#### Scenario: Mutable receiver
- **WHEN** a method declares `&mut self`
- **THEN** its implementation may mutate the receiver through exclusive access

#### Scenario: Trait receiver mismatch
- **WHEN** a trait declares `&self` and an implementation declares `self` or `&mut self`
- **THEN** conformance checking rejects the implementation

### Requirement: Receiver effects are visible at calls
A bound mutable receiver SHALL be called with receiver modifier `&mut`, and a
bound consuming receiver SHALL be called with `move`. A shared receiver SHALL
auto-borrow without a modifier. The modifier SHALL apply before the receiver's
postfix call chain.

#### Scenario: Mutable method call
- **WHEN** source calls `&mut user.rename(name)` on a mutable owner
- **THEN** the selected `&mut self` method receives an exclusive borrow

#### Scenario: Consuming method call
- **WHEN** source calls `move user.finish()` for a method taking `self`
- **THEN** ownership moves into the method and later use of `user` is rejected

#### Scenario: Missing receiver modifier
- **WHEN** a bound owner calls an `&mut self` or consuming method without its required modifier
- **THEN** checking reports the missing ownership mode at the receiver
