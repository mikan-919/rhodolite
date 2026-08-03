## Purpose

Lets programs pass named top-level functions to higher-order code while keeping
the callback target and its inferred ambient requirements statically visible.

## Requirements

### Requirement: Named top-level functions have callable value types
The system SHALL support a callable type annotation written `fn(P1, P2 -> R)`,
where every parameter type and the result type are existing concrete Rhodolite
types. A reference to a named top-level function in a value position SHALL
produce that function's callable value when its declared signature exactly
matches the expected callable type. Callable values SHALL be Copy values and
MUST NOT capture local bindings, ambient provisions, or receiver state.

#### Scenario: A named function is passed as a callback
- **WHEN** a top-level function expects `fn(int -> int)` and is called with the
  name of a top-level `fn double(value: int -> int)`
- **THEN** the call type-checks and the callback may be invoked by its parameter

#### Scenario: A callable local can be reused
- **WHEN** a named top-level function is bound to an immutable local with a
  matching callable type and that local is passed or called more than once
- **THEN** each use refers to the same named function without moving it

#### Scenario: Callable signatures must match exactly
- **WHEN** a named top-level function is used where a callable type differs in
  arity, parameter type or ownership mode, or result type
- **THEN** type checking fails at the function-value use with the expected and
  actual callable signatures

### Requirement: Indirect calls target statically known named functions
The system SHALL allow a local or parameter with a callable type to be invoked
using ordinary call syntax. Every successful indirect call SHALL resolve, for
the enclosing call specialization, to one named top-level function supplied by
the caller. The system SHALL reject a callable value placed in a mutable local,
field, enum payload, array element, or return value in this change.

#### Scenario: Higher-order helper invokes its callback
- **WHEN** `apply` accepts `fn(int -> int)` and evaluates `f(value)` through its
  callable parameter
- **THEN** `apply(double, 21)` evaluates to `42` through the named `double`
  function

#### Scenario: Unsupported callable storage is rejected
- **WHEN** a program attempts to assign a callable value to a struct field,
  enum payload, array element, mutable local, or function return value
- **THEN** type checking rejects the use before ownership checking or execution

### Requirement: Ambient requirements flow through indirect calls
The system SHALL infer ambient requirements through a higher-order helper from
the named callback selected at each call site. A helper called with callbacks
that require different slots SHALL be treated as separate call specializations
for analysis and generated-code purposes, without exposing effect variables in
source-level types.

#### Scenario: Callback requirement reaches the caller
- **WHEN** a callback passed to `apply` calls an ambient `db` slot and `apply`
  invokes that callback
- **THEN** the caller of `apply` is inferred to require `db` unless it provides
  `db` around that call

#### Scenario: Missing provider reports the indirect path
- **WHEN** a production root reaches a callback requirement through an indirect
  call without a matching `with` provision
- **THEN** compilation fails before Wasm generation with a path that includes
  the higher-order helper and selected callback

#### Scenario: Callback specializations remain distinct
- **WHEN** the same helper is called once with a callback requiring `db` and
  once with a callback requiring no ambient slot
- **THEN** the no-slot call does not acquire a hidden `db` argument or a
  provider requirement from the other call

### Requirement: Both execution paths preserve indirect-call behavior
The checked-HIR interpreter and generated Core Wasm module SHALL execute the
same statically resolved named callback for every supported indirect call.
The generated module SHALL remain deterministic and SHALL require no new host
imports or public ABI surface for callable values used only internally.

#### Scenario: Interpreter and Wasm agree on a callback result
- **WHEN** a supported program passes a named callback through a higher-order
  helper and executes a production root
- **THEN** the interpreter and independent Wasm engine produce equal observable
  outcomes

#### Scenario: Repeated compilation preserves callable specialization bytes
- **WHEN** a program containing supported indirect calls is compiled twice with
  identical sources and options
- **THEN** the generated Wasm byte sequences are identical
