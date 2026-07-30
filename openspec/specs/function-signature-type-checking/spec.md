# function-signature-type-checking

## Purpose

Carry type facts across the function-call boundary. Direct top-level calls are
checked against their declared signature, their result carries the declared
return type into later checks, and declared returns are checked against the
values a function actually yields — always limited to expressions the checker
can infer.

## Requirements

### Requirement: Direct top-level calls enforce declared arity
The checker SHALL compare the argument count of a direct top-level function call with the called function's declared parameter count.

#### Scenario: Matching argument count
- **WHEN** a direct top-level function is called with its declared number of arguments
- **THEN** the checker produces no arity diagnostic for that call

#### Scenario: Too few arguments
- **WHEN** a direct top-level function is called with fewer arguments than declared
- **THEN** the checker reports the called function and the expected and actual argument counts

#### Scenario: Too many arguments
- **WHEN** a direct top-level function is called with more arguments than declared
- **THEN** the checker reports the called function and the expected and actual argument counts

### Requirement: Direct top-level calls check argument types
The checker SHALL compare every argument type with the corresponding declared parameter type using directional destination compatibility. Exact nominal identity and optionality SHALL be compatible. A non-optional `T` argument SHALL also be compatible with a parameter of the same nominal type `T?`. An optional `T?` argument SHALL NOT be compatible with a non-optional `T` parameter. Failure to determine an argument type SHALL fail checking.

#### Scenario: Matching argument type
- **WHEN** a direct top-level function parameter and its argument have the same nominal type and optionality
- **THEN** the checker produces no argument-type diagnostic

#### Scenario: Present argument for an optional parameter
- **WHEN** a direct top-level function parameter has type `T?` and its argument has type `T`
- **THEN** the checker produces no argument-type diagnostic and the argument remains typed as `T`

#### Scenario: Mismatched argument type
- **WHEN** a direct top-level function parameter and its argument have incompatible nominal types or optionality
- **THEN** the checker reports the called function, argument position, expected type, and actual type

#### Scenario: Argument type cannot be determined
- **WHEN** the checker cannot determine an argument expression's type
- **THEN** checking fails at that argument before evaluation

### Requirement: Direct call results carry return types
The checker SHALL assign the declared return type to a direct top-level function call. A function without a return annotation SHALL have return type `unit`, and its calls SHALL have type `unit`.

#### Scenario: Bind a direct call result
- **WHEN** a direct top-level function with a declared return type is called and its result is bound to a local
- **THEN** subsequent references to that local have the declared return type

#### Scenario: Direct call result reaches an existing field check
- **WHEN** a direct call's declared enum return type is incompatible with a known enum-typed struct field
- **THEN** the existing field-type check reports the mismatch

#### Scenario: Function has no return annotation
- **WHEN** a direct top-level function without a return annotation is called
- **THEN** the call has type `unit`

### Requirement: Function bodies satisfy their return types
The checker SHALL compare every value-carrying explicit return and every value-producing final body expression with the function's effective return type using directional destination compatibility. The effective return type SHALL be the annotation when present and `unit` otherwise. A non-optional `T` result SHALL satisfy `T?`, while `T?` SHALL NOT satisfy `T`. Failure to determine a returned expression's type SHALL fail checking.

#### Scenario: Matching final expression
- **WHEN** a function's final expression has a type matching its effective return type
- **THEN** the checker produces no return-type diagnostic

#### Scenario: Present final expression for an optional return
- **WHEN** a function declares return type `T?` and its final expression has type `T`
- **THEN** checking succeeds and the expression remains typed as `T`

#### Scenario: Present explicit return for an optional return
- **WHEN** a function declares return type `T?` and an explicit `return` carries a value of type `T`
- **THEN** the checker produces no return-type diagnostic

#### Scenario: Mismatched final expression
- **WHEN** a function's final expression is incompatible with its effective return type
- **THEN** the checker reports the function, expected return type, and actual type

#### Scenario: Mismatched explicit return
- **WHEN** an explicit `return` carries a type incompatible with the containing function's effective return type
- **THEN** the checker reports the function, expected return type, and actual type

#### Scenario: Returned expression type cannot be determined
- **WHEN** the checker cannot determine an explicit or final returned expression's type
- **THEN** checking fails at that expression

#### Scenario: Omitted annotation returns unit
- **WHEN** a function without a return annotation ends in a non-`unit` value
- **THEN** checking fails with expected type `unit`

### Requirement: Unsupported call forms remain outside signature checking
The checker SHALL limit this capability to direct top-level function calls. Method and associated-function calls SHALL be governed by the dedicated `method-call-type-checking` capability instead of being left without an inferred signature.

#### Scenario: Method call
- **WHEN** a call uses a field receiver such as `value.method()`
- **THEN** this capability leaves the call to `method-call-type-checking`, which resolves and checks it when the receiver or slot is known

#### Scenario: Associated-function call
- **WHEN** a call uses a path such as `Type::make()`
- **THEN** this capability leaves the call to `method-call-type-checking`, which resolves and checks a known type or slot path
