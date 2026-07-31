# integer-runtime-semantics

## Purpose

Defines one backend-independent meaning for Rhodolite integer arithmetic so
interpreted and compiled programs cannot differ because of host build settings.

## Requirements

### Requirement: Int is a signed 64-bit value
The `int` type SHALL contain exactly the signed two's-complement 64-bit values
from `-9223372036854775808` through `9223372036854775807`. Integer behavior
SHALL NOT depend on whether the Rhodolite implementation itself was built with
overflow checks enabled.

#### Scenario: Boundary values are representable
- **WHEN** evaluation produces either the minimum or maximum signed 64-bit integer
- **THEN** the value remains a valid `int`

### Requirement: Basic integer overflow wraps
Integer addition, subtraction, multiplication, and unary negation SHALL wrap
modulo 2^64 and reinterpret the resulting bit pattern as a signed
two's-complement value.

#### Scenario: Addition wraps above the maximum
- **WHEN** `9223372036854775807 + 1` is evaluated
- **THEN** the result is `-9223372036854775808`

#### Scenario: Subtraction wraps below the minimum
- **WHEN** `-9223372036854775808 - 1` is evaluated
- **THEN** the result is `9223372036854775807`

#### Scenario: Negating the minimum wraps
- **WHEN** unary negation is applied to `-9223372036854775808`
- **THEN** the result is `-9223372036854775808`

### Requirement: Signed division has explicit failure cases
Integer division SHALL truncate toward zero. Division by zero and division of
the minimum signed 64-bit integer by `-1` SHALL be runtime failures rather than
host panics or wrapped results.

#### Scenario: Signed division truncates toward zero
- **WHEN** `-7 / 2` is evaluated
- **THEN** the result is `-3`

#### Scenario: Division by zero fails
- **WHEN** an integer is divided by zero
- **THEN** evaluation reports a runtime failure

#### Scenario: Minimum divided by negative one fails
- **WHEN** `-9223372036854775808 / -1` is evaluated
- **THEN** evaluation reports a runtime failure

### Requirement: Every execution backend shares integer results
For each successfully evaluated integer expression, the interpreter and every
compiled backend SHALL produce the same signed 64-bit result. The failure cases
defined by this capability SHALL fail in every backend, although each backend
MAY transport or render the runtime failure differently.

#### Scenario: Wrapping expression is interpreted and compiled
- **WHEN** the same overflowing arithmetic expression is run by the interpreter and a compiled backend
- **THEN** both executions produce the same wrapped value

#### Scenario: Division failure uses backend-specific transport
- **WHEN** a defined division failure is reached in interpreted and compiled execution
- **THEN** both executions fail even if the interpreter reports a diagnostic and the compiled artifact traps
