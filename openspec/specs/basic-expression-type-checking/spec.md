# basic-expression-type-checking

## Purpose

Give the smallest expressions a type. Scalar literals, ordinary non-optional
struct-field reads, primitive operators, boolean contexts, and known
assignments are checked before evaluation, on top of four reserved built-in
scalar type names. Optional values, arrays, and method or associated-function
resolution stay outside this boundary and are deferred rather than treated as
a wildcard type.

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

### Requirement: Known ordinary field reads use struct declarations
For an ordinary field expression `receiver.field` whose receiver has a known non-optional type, the checker SHALL require that type to name a loaded struct and SHALL require the struct to declare the field. A valid read SHALL have the field's declared type.

#### Scenario: Read a declared field
- **WHEN** `user` has known non-optional struct type `User` and `User` declares `rank: Rank`
- **THEN** `user.rank` passes field checking and has type `Rank`

#### Scenario: Chain declared field reads
- **WHEN** each receiver in a non-optional field chain has a known struct type and each field is declared
- **THEN** the checker propagates each declared field type through the chain

#### Scenario: Read a missing field
- **WHEN** a known struct receiver is used with a field absent from its declaration
- **THEN** checking fails with a diagnostic identifying the receiver type and missing field

#### Scenario: Read a field from a known non-struct
- **WHEN** a receiver has a known non-optional type that is not a loaded struct
- **THEN** checking fails instead of treating the receiver as a struct

#### Scenario: Receiver type is temporarily unknown
- **WHEN** a field receiver is an expression form outside this capability's inference boundary
- **THEN** this capability defers the receiver and produces no field-type diagnostic

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

### Requirement: Equality requires matching known operand types
The checker SHALL infer `bool` for every equality expression. When both operand types are known, it SHALL require exact nominal type and optionality equality.

#### Scenario: Compare matching known types
- **WHEN** both operands of `==` have the same known type and optionality
- **THEN** the comparison passes operand checking and has type `bool`

#### Scenario: Compare mismatched known types
- **WHEN** both operands of `==` have different known nominal types or optionality
- **THEN** checking fails with a diagnostic identifying both operand types

#### Scenario: One equality operand is temporarily unknown
- **WHEN** at least one operand of `==` is outside this capability's inference boundary
- **THEN** the checker defers operand compatibility but still infers the comparison result as `bool`

### Requirement: Boolean contexts reject known non-booleans
The checker SHALL require every inferable `if`, `elif`, and `while` condition and every inferable `assert` operand to have type `bool`.

#### Scenario: Boolean control-flow condition
- **WHEN** an `if`, `elif`, or `while` condition has type `bool`
- **THEN** the checker produces no condition-type diagnostic

#### Scenario: Known non-boolean control-flow condition
- **WHEN** an `if`, `elif`, or `while` condition has a known type other than `bool`
- **THEN** checking fails with a diagnostic requiring `bool`

#### Scenario: Boolean assertion
- **WHEN** an `assert` operand has type `bool`
- **THEN** the checker produces no assertion-type diagnostic

#### Scenario: Known non-boolean assertion
- **WHEN** an `assert` operand has a known type other than `bool`
- **THEN** checking fails with a diagnostic requiring `bool`

#### Scenario: Boolean-context expression is temporarily unknown
- **WHEN** a condition or assertion operand is outside this capability's inference boundary
- **THEN** this capability defers the type check

### Requirement: Known assignments preserve exact types
The checker SHALL compare inferable struct-literal field values, field-assignment values, and ordinary-binding reassignments with their established destination types using exact nominal identity and optionality.

#### Scenario: Matching struct-literal field value
- **WHEN** a struct literal supplies an inferable value whose type exactly matches the declared field type
- **THEN** the checker produces no field-type diagnostic

#### Scenario: Mismatched struct-literal field value
- **WHEN** a struct literal supplies an inferable value whose type differs from the declared field type
- **THEN** checking fails with a diagnostic identifying the struct, field, expected type, and actual type

#### Scenario: Matching field assignment
- **WHEN** a known struct field receives an inferable value whose type exactly matches its declaration
- **THEN** the checker produces no assignment-type diagnostic

#### Scenario: Mismatched field assignment
- **WHEN** a known struct field receives an inferable value whose type differs from its declaration
- **THEN** checking fails with a diagnostic identifying the field, expected type, and actual type

#### Scenario: Matching local reassignment
- **WHEN** an ordinary binding with an established type is reassigned an inferable value of exactly that type
- **THEN** the checker produces no reassignment-type diagnostic

#### Scenario: Mismatched local reassignment
- **WHEN** an ordinary binding with an established type is reassigned an inferable value of a different type
- **THEN** checking fails with a diagnostic identifying the binding, expected type, and actual type

#### Scenario: Assignment source is temporarily unknown
- **WHEN** an assignment source is outside this capability's inference boundary
- **THEN** this capability defers compatibility checking

#### Scenario: Binding initializer is temporarily unknown
- **WHEN** a binding's initializer is outside this capability's inference boundary
- **THEN** later assignments do not establish a flow-sensitive type for that binding

### Requirement: Unsupported expression forms remain deferred, not dynamically typed
This capability SHALL leave arrays, optional field access, method calls, and associated-function calls outside its inference boundary. The checker SHALL treat this as a temporary implementation limitation rather than as a language-level wildcard or dynamic type. Core optional expressions `nil` and `??` are governed by the optional-core-type-checking capability and are no longer deferred by this requirement.

#### Scenario: Unsupported expression reaches a compatibility check
- **WHEN** an array expression, optional field access, method call, or associated-function call reaches a check added by this capability
- **THEN** this capability produces no mismatch diagnostic solely because that expression has no inferred type

#### Scenario: No implicit wildcard compatibility
- **WHEN** a later capability adds a type for a previously unsupported expression form
- **THEN** the expression participates in the same exact compatibility rules without a wildcard exception

### Requirement: Repository sources use canonical scalar spellings
The canonical program, maintained examples, tests, and type-system documentation SHALL use lowercase built-in scalar names for scalar values and SHALL NOT rely on undeclared `UserId` or `Time` names as integer aliases.

#### Scenario: Canonical scalar values
- **WHEN** the canonical program is checked after migration
- **THEN** identifiers, timestamps, and counters use `int`, boolean results use `bool`, strings use `str`, and the canonical test still succeeds
