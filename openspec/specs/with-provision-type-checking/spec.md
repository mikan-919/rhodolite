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

### Requirement: Unknown provision types fail static checking
The checker SHALL require every `with slot(value)` provision for a declared slot to have a concrete inferred value type and SHALL compare that type with the slot trait before evaluation. A provision value whose type cannot be determined SHALL fail checking. A head name that is not a declared slot SHALL continue to use the existing undeclared-slot diagnostic.

#### Scenario: Provision value has no type
- **WHEN** a `with slot(value)` provision's value type cannot be determined
- **THEN** checking fails at the provision value instead of deferring to evaluation

#### Scenario: Head name is not a slot
- **WHEN** a `with` head names something that is not a declared slot
- **THEN** checking fails through the existing undeclared-slot diagnostic

### Requirement: Canonical provisions pass statically
The canonical program SHALL keep passing checking and execution with its provisions validated before it runs.

#### Scenario: Canonical production and test provisions
- **WHEN** the canonical program is checked
- **THEN** `Postgres` and `SystemClock` in `main`, and `InMemoryDb` and `Frozen` in the test, are accepted as implementations of `Database` and `Clock`

#### Scenario: Canonical behavior is unchanged
- **WHEN** the canonical main program and test are executed
- **THEN** their existing successful results remain unchanged
