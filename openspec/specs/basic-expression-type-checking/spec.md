# basic-expression-type-checking

## Purpose

Give the smallest expressions a type. Scalar literals, ordinary and optional
struct-field reads, primitive operators, boolean contexts, and known
assignments are checked before evaluation, on top of four reserved built-in
scalar type names. Arrays and method or associated-function resolution stay
outside this boundary and are deferred rather than treated as a wildcard type.

## Requirements

### Requirement: Scalar literals have reserved built-in types
The language SHALL define `int`, `bool`, `str`, and `unit` as reserved built-in type names that loaded user declarations cannot override. Integer, boolean, and string literals SHALL have the exact types `int`, `bool`, and `str`, respectively.

#### Scenario: Infer scalar literal types
- **WHEN** integer, boolean, and string literals are checked
- **THEN** their inferred types are `int`, `bool`, and `str`, respectively

#### Scenario: Built-in type names cannot be redeclared
- **WHEN** a loaded user declaration attempts to declare `int`, `bool`, `str`, or `unit`
- **THEN** checking fails with a deterministic diagnostic identifying the reserved name

#### Scenario: Literal type reaches a direct call
- **WHEN** a scalar literal is passed to a direct top-level function with a declared parameter type
- **THEN** the existing signature check compares the literal's built-in type with the parameter type

#### Scenario: Literal type reaches a declared return
- **WHEN** a scalar literal is returned from a function with a declared return type
- **THEN** the existing return check compares the literal's built-in type with the declared type

### Requirement: Ordinary field reads use struct declarations
For every ordinary field expression `receiver.field`, the checker SHALL require the receiver to have a concrete non-optional type, SHALL require that type to name a loaded struct, and SHALL require the struct to declare the field. A valid read SHALL have the field's declared type. An optional receiver SHALL fail and require explicit optional access or prior fallback. Failure to determine the receiver type SHALL fail checking.

#### Scenario: Read a declared field
- **WHEN** `user` has non-optional struct type `User` and `User` declares `rank: Rank`
- **THEN** `user.rank` passes field checking and has type `Rank`

#### Scenario: Chain declared field reads
- **WHEN** each receiver in a non-optional field chain has a concrete struct type and each field is declared
- **THEN** the checker propagates each declared field type through the chain

#### Scenario: Read a missing field
- **WHEN** a struct receiver is used with a field absent from its declaration
- **THEN** checking fails with a diagnostic identifying the receiver type and missing field

#### Scenario: Read a field from a non-struct
- **WHEN** a receiver has a non-optional type that is not a loaded struct
- **THEN** checking fails instead of treating the receiver as a struct

#### Scenario: Read an ordinary field from an optional receiver
- **WHEN** an ordinary field receiver has optional type
- **THEN** checking fails and identifies explicit optional access or prior fallback as the valid alternatives

#### Scenario: Receiver type cannot be determined
- **WHEN** the checker cannot determine a field receiver's type
- **THEN** checking fails at the receiver instead of deferring the field check

### Requirement: Arithmetic and negation require integers
The checker SHALL type `+`, `-`, `*`, and `/` as `int × int -> int` and unary `-` as `int -> int`. It SHALL NOT provide implicit scalar conversions or string concatenation through these operators.

#### Scenario: Integer arithmetic
- **WHEN** both operands of an arithmetic operator have type `int`
- **THEN** the expression passes operand checking and has type `int`

#### Scenario: Known non-integer arithmetic operand
- **WHEN** either arithmetic operand has a known type other than `int`
- **THEN** checking fails with a diagnostic identifying the operator and incompatible type

#### Scenario: Negate an integer
- **WHEN** unary `-` receives an `int`
- **THEN** the expression passes operand checking and has type `int`

#### Scenario: Negate a known non-integer
- **WHEN** unary `-` receives a known type other than `int`
- **THEN** checking fails with an incompatible-operand diagnostic

#### Scenario: String addition is not concatenation
- **WHEN** `+` receives a known `str` operand
- **THEN** checking fails instead of treating `+` as string concatenation

### Requirement: Equality requires matching operand types
The checker SHALL infer `bool` for every valid equality expression and SHALL require both operands to have concrete, exactly matching nominal types and optionality, except for the dedicated contextual `nil` rules. Failure to determine either operand type SHALL fail checking.

#### Scenario: Compare matching types
- **WHEN** both operands of `==` have the same concrete type and optionality
- **THEN** the comparison passes operand checking and has type `bool`

#### Scenario: Compare mismatched types
- **WHEN** both operands of `==` have different nominal types or optionality
- **THEN** checking fails with a diagnostic identifying both operand types

#### Scenario: Equality operand type cannot be determined
- **WHEN** the checker cannot determine at least one operand type and no contextual `nil` rule determines it
- **THEN** checking fails at the unknown operand

### Requirement: Boolean contexts require booleans
The checker SHALL require every `if`, `elif`, and `while` condition and every `assert` operand to have concrete type `bool`. Failure to determine the condition or operand type SHALL fail checking.

#### Scenario: Boolean control-flow condition
- **WHEN** an `if`, `elif`, or `while` condition has type `bool`
- **THEN** the checker produces no condition-type diagnostic

#### Scenario: Non-boolean control-flow condition
- **WHEN** an `if`, `elif`, or `while` condition has a type other than `bool`
- **THEN** checking fails with a diagnostic requiring `bool`

#### Scenario: Boolean assertion
- **WHEN** an `assert` operand has type `bool`
- **THEN** the checker produces no assertion-type diagnostic

#### Scenario: Non-boolean assertion
- **WHEN** an `assert` operand has a type other than `bool`
- **THEN** checking fails with a diagnostic requiring `bool`

#### Scenario: Boolean-context type cannot be determined
- **WHEN** the checker cannot determine a condition or assertion operand's type
- **THEN** checking fails at that expression

### Requirement: Assignments preserve exact types
The checker SHALL compare every struct-literal field value, field-assignment value, and ordinary-binding reassignment with its established destination type using directional compatibility. Exact nominal identity and optionality SHALL be compatible. A non-optional `T` value SHALL also be compatible with `T?`. An optional `T?` value SHALL NOT be compatible with `T`. Failure to determine either source or destination type SHALL fail checking.

#### Scenario: Matching struct-literal field value
- **WHEN** a struct literal supplies a value whose type exactly matches the declared field type
- **THEN** the checker produces no field-type diagnostic

#### Scenario: Present value initializes an optional field
- **WHEN** a struct field is declared as `T?` and its initializer has type `T`
- **THEN** checking succeeds without changing the initializer's type

#### Scenario: Mismatched struct-literal field value
- **WHEN** a struct literal supplies a value incompatible with the declared field type
- **THEN** checking fails with a diagnostic identifying the struct, field, expected type, and actual type

#### Scenario: Matching field assignment
- **WHEN** a struct field receives a value compatible with its declaration
- **THEN** the checker produces no assignment-type diagnostic

#### Scenario: Mismatched field assignment
- **WHEN** a struct field receives a value incompatible with its declaration
- **THEN** checking fails with a diagnostic identifying the field, expected type, and actual type

#### Scenario: Matching local reassignment
- **WHEN** an ordinary binding with an established type is reassigned a compatible value
- **THEN** the checker produces no reassignment-type diagnostic

#### Scenario: Mismatched local reassignment
- **WHEN** an ordinary binding with an established type is reassigned an incompatible value
- **THEN** checking fails with a diagnostic identifying the binding, expected type, and actual type

#### Scenario: Optional value does not flow into a required destination
- **WHEN** a field or established local has type `T` and receives a value of type `T?`
- **THEN** checking fails with the ordinary assignment mismatch diagnostic

#### Scenario: Assignment source type cannot be determined
- **WHEN** the checker cannot determine an assignment source type
- **THEN** checking fails at the source expression

#### Scenario: Unannotated initializer type cannot be determined
- **WHEN** the checker cannot determine an unannotated binding initializer's type
- **THEN** checking fails and the binding is not introduced with an unknown type

### Requirement: Every supported expression form participates in total checking
Every expression form accepted by the parser SHALL be typed by this capability or by a dedicated type-checking capability. The checker SHALL NOT successfully defer an accepted expression as unsupported, dynamically typed, or wildcard typed.

#### Scenario: Accepted expression has no typing rule
- **WHEN** an accepted expression reaches checking without a typing rule
- **THEN** checking fails at that expression

#### Scenario: Dedicated capability supplies a type
- **WHEN** a dedicated capability types an array, optional expression, match, method call, or associated-function call
- **THEN** the expression participates in ordinary destination and operand compatibility rules

### Requirement: Repository sources use canonical scalar spellings
The canonical program, maintained examples, tests, and type-system documentation SHALL use lowercase built-in scalar names for scalar values and SHALL NOT rely on undeclared `UserId` or `Time` names as integer aliases.

#### Scenario: Canonical scalar values
- **WHEN** the canonical program is checked after migration
- **THEN** identifiers, timestamps, and counters use `int`, boolean results use `bool`, strings use `str`, and the canonical test still succeeds
