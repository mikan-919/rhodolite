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

### Requirement: Direct top-level calls check known argument types
The checker SHALL compare each inferable argument type with the corresponding declared parameter type using directional destination compatibility. Exact nominal identity and optionality SHALL be compatible. A non-optional `T` argument SHALL also be compatible with a parameter of the same nominal type `T?`. An optional `T?` argument SHALL NOT be compatible with a non-optional `T` parameter.

#### Scenario: Matching known argument type
- **WHEN** a direct top-level function parameter and its inferable argument have the same nominal type and optionality
- **THEN** the checker produces no argument-type diagnostic

#### Scenario: Present argument for an optional parameter
- **WHEN** a direct top-level function parameter has type `T?` and its inferable argument has type `T`
- **THEN** the checker produces no argument-type diagnostic and the argument remains inferred as `T`

#### Scenario: Mismatched known argument type
- **WHEN** a direct top-level function parameter and its inferable argument have incompatible nominal types or optionality
- **THEN** the checker reports the called function, argument position, expected type, and actual type

#### Scenario: Argument type is unknown
- **WHEN** an argument expression is outside the change's inference boundary
- **THEN** the checker produces no argument-type diagnostic for that argument

### Requirement: Direct call results carry declared return types
The checker SHALL infer the declared return type of a direct top-level function call and make that type available to subsequent checks.

#### Scenario: Bind a direct call result
- **WHEN** a direct top-level function with a declared return type is called and its result is bound to a local
- **THEN** subsequent references to that local have the declared return type

#### Scenario: Direct call result reaches an existing field check
- **WHEN** a direct call's declared enum return type is incompatible with a known enum-typed struct field
- **THEN** the existing field-type check reports the mismatch

#### Scenario: Function has no declared return type
- **WHEN** a direct top-level function without a return annotation is called
- **THEN** the checker treats the call result type as unknown

### Requirement: Declared function returns check known values
For a function with a declared return type, the checker SHALL compare inferable explicit return values and the inferable final body expression with the declared return type using directional destination compatibility. A non-optional `T` result SHALL satisfy a return type of the same nominal type `T?`, while an optional `T?` result SHALL NOT satisfy a non-optional `T` return type.

#### Scenario: Matching final expression
- **WHEN** a function's final expression has an inferable type matching its declared return type
- **THEN** the checker produces no return-type diagnostic

#### Scenario: Present final expression for an optional return
- **WHEN** a function declares return type `T?` and its final expression has inferable type `T`
- **THEN** the checker produces no return-type diagnostic and the expression remains inferred as `T`

#### Scenario: Present explicit return for an optional return
- **WHEN** a function declares return type `T?` and an explicit `return` carries an inferable value of type `T`
- **THEN** the checker produces no return-type diagnostic

#### Scenario: Mismatched final expression
- **WHEN** a function's final expression has an inferable type incompatible with its declared return type
- **THEN** the checker reports the function, expected return type, and actual type

#### Scenario: Mismatched explicit return
- **WHEN** an explicit `return` carries an inferable type incompatible with the containing function's declared return type
- **THEN** the checker reports the function, expected return type, and actual type

#### Scenario: Returned expression type is unknown
- **WHEN** an explicit or final returned expression is outside the change's inference boundary
- **THEN** the checker produces no return-type diagnostic for that expression

### Requirement: Unsupported call forms remain outside signature checking
The checker SHALL limit this capability to direct top-level function calls and SHALL NOT infer method or associated-function results through this capability.

#### Scenario: Method call
- **WHEN** a call uses a field receiver such as `value.method()`
- **THEN** this capability produces no arity, argument-type, or return-type diagnostic for the method signature

#### Scenario: Associated-function call
- **WHEN** a call uses a path such as `Type::make()`
- **THEN** this capability produces no arity, argument-type, or return-type diagnostic for the associated-function signature
