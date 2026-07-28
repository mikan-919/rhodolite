# with-provision-type-checking

## Purpose

Check that a `with` provision satisfies the contract its slot declares, before
the program runs. A slot is a named implementation choice, so the value or type
placed into it must implement the trait the `effect` declaration names. The
evaluator keeps the same judgement as a runtime net for provisions whose type
the checker cannot infer.

## Requirements

### Requirement: Provisions satisfy the slot's declared trait
The checker SHALL require every `with` provision whose slot is declared by an `effect` to name a concrete type that implements the slot's trait. A `with slot<Type>` provision SHALL be checked from the named type. A `with slot(value)` provision SHALL be checked from the value's inferred type. A provision type that is optional, an array, a builtin scalar, or a type without an `impl Trait for Type` declaration SHALL be reported.

#### Scenario: Implementing type provided by value
- **WHEN** a `with slot(value)` provision's value has a concrete type that implements the slot's trait
- **THEN** the checker produces no provision diagnostic

#### Scenario: Implementing type provided by name
- **WHEN** a `with slot<Type>` provision names a type that implements the slot's trait
- **THEN** the checker produces no provision diagnostic

#### Scenario: Non-implementing struct
- **WHEN** a provision's type is a declared struct with no `impl` of the slot's trait
- **THEN** the checker reports the provided type, the required trait, and the slot

#### Scenario: Scalar or optional provision
- **WHEN** a provision's inferred type is a builtin scalar, an array, or an optional type
- **THEN** the checker reports it as not implementing the slot's trait

#### Scenario: Provision inside an unexecuted block
- **WHEN** a non-conforming provision appears in a `with` block whose body never runs
- **THEN** the checker still reports it, because the check happens at the declaration

### Requirement: Unknown provision types defer to the runtime check
The checker SHALL skip a `with slot(value)` provision whose value type it cannot infer, and SHALL skip any provision whose head name is not a declared slot. The evaluator SHALL keep rejecting a provision that does not implement the slot's trait when it is reached at run time.

#### Scenario: Provision value outside the inference boundary
- **WHEN** a `with slot(value)` provision's value has no inferable type
- **THEN** the checker defers that provision and the evaluator performs the check when the `with` head is evaluated

#### Scenario: Head name is not a slot
- **WHEN** a `with` head names something that is not a declared slot
- **THEN** the provision check produces no diagnostic and the existing undeclared-slot report stands

### Requirement: Canonical provisions pass statically
The canonical program SHALL keep passing checking and execution with its provisions validated before it runs.

#### Scenario: Canonical production and test provisions
- **WHEN** the canonical program is checked
- **THEN** `Postgres` and `SystemClock` in `main`, and `InMemoryDb` and `Frozen` in the test, are accepted as implementations of `Database` and `Clock`

#### Scenario: Canonical behavior is unchanged
- **WHEN** the canonical main program and test are executed
- **THEN** their existing successful results remain unchanged
