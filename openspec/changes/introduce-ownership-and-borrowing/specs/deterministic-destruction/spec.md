## Purpose

Defines deterministic compiler-generated destruction for owned data so resource
and buffer lifetimes follow lexical ownership without tracing garbage collection.

## ADDED Requirements

### Requirement: Owned values are dropped deterministically
An initialized owned local SHALL be dropped when its lexical scope exits, in
reverse declaration order. A value moved from a place SHALL not be dropped at
that source place. Dropping a composite SHALL recursively drop its still-owned
contents exactly once.

#### Scenario: Reverse declaration order
- **WHEN** two owned locals remain initialized at the end of one scope
- **THEN** the later declaration is dropped before the earlier declaration

#### Scenario: Moved local is not dropped twice
- **WHEN** ownership moves from one local to another before scope exit
- **THEN** only the destination owner drops the value

#### Scenario: Composite drop
- **WHEN** an owned struct containing strings and arrays is dropped
- **THEN** every owned field and buffer is dropped exactly once

### Requirement: Structured exits preserve cleanup
Normal fallthrough, `return`, branch exit, and loop exit SHALL drop every owned
value whose scope is exited. A runtime trap SHALL not perform language-level
unwinding; destruction after a trap is delegated to disposal of the Wasm
instance or interpreter execution context.

#### Scenario: Early return
- **WHEN** a function returns from inside a nested block
- **THEN** initialized owned locals in every exited scope are dropped before returning

#### Scenario: Trap does not unwind
- **WHEN** an assertion or arithmetic failure traps during compiled execution
- **THEN** no user-observable destructor unwinding is attempted

### Requirement: Destruction is compiler-defined only
This version SHALL NOT expose a user-defined destructor, finalizer, or unsafe
manual-free hook. The compiler MAY release storage after the last use rather
than the lexical endpoint only when doing so cannot change observable behavior.

#### Scenario: User destructor syntax is unavailable
- **WHEN** source attempts to declare a custom drop or finalizer hook
- **THEN** the declaration is rejected as unsupported syntax
