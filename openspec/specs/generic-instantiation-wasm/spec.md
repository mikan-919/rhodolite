## Purpose

Fixes, as an explicit and tested contract, the boundary between generic
instantiation (MAP-020, MAP-025, MAP-040, MAP-050) and the Core Wasm backend
(`src/wasm.rs`): the backend compiles any reached generic function or
generic trait method instantiation to one directly-called Wasm function
exactly as it would compile a hand-written callable, with no table,
`funcref`, closure allocation, or new host import, and with a return value,
failure classification, and owned-value final state matching the
interpreter.

## Requirements

### Requirement: The Wasm backend compiles a generic instantiation like any other callable
The backend SHALL compile a generic function's or generic trait method's
instantiation with no case, branch, or special handling for "this instance
came from a generic declaration" — an instantiation's compiled function
SHALL be indistinguishable, in shape and in how it is reached (direct,
associated, method, or slot/ambient call), from a callable written without
type parameters.

#### Scenario: A generic function instantiation compiles and runs to completion
- **WHEN** a program's entry point calls `identity<T>(x: T -> T) { x }` and
  returns its result
- **THEN** the generated Core Wasm module runs to completion in an
  independent engine and produces the argument's value unchanged

#### Scenario: A generic trait method instantiation compiles and runs to completion
- **WHEN** a program declares a generic trait, a generic impl of it for a
  concrete struct, and calls the resulting generic method through an
  ordinary method-call expression
- **THEN** the generated module resolves the call to the method's concrete
  instantiation's compiled function and runs it to completion, producing the
  same result a hand-written (non-generic) method with the same body would
  produce

### Requirement: Only reached generic instantiation instances are compiled
The backend SHALL compile exactly one Wasm function for each generic
instantiation instance reached from a production root, and SHALL compile no
function for a generic declaration, or a combination of type arguments,
callback bindings, or providers, that no reachable call site produces.

#### Scenario: An unreached generic declaration adds no function
- **WHEN** a program declares a generic function that no production root
  transitively calls
- **THEN** the generated module contains no compiled function for that
  declaration

#### Scenario: Reached type-argument, callback, and provider combinations each compile once
- **WHEN** a program calls the same generic declaration at two distinct
  combinations of type arguments, callback binding, or ambient provider
- **THEN** the generated module contains one compiled function per distinct
  reached combination, and no combination is compiled more than once

### Requirement: Distinct instantiation instances lower to deterministic direct calls
Where a call site's callee is a generic instantiation, the backend SHALL
resolve that call to one statically known Wasm function index at
compile time and SHALL emit a direct call to it. The backend SHALL NOT
introduce a function table, a `funcref`-typed value, a closure allocation,
or a new host import to compile any generic instantiation or any call to
one.

#### Scenario: A call to a generic instantiation is a direct call
- **WHEN** a program calls a generic function or generic trait method
  instantiation
- **THEN** the generated module's call site is a direct call by function
  index, and the module declares no function table and imports nothing
  beyond what an equivalent non-generic program would import

#### Scenario: Two callback bindings of the same generic declaration resolve to their own targets
- **WHEN** a generic function taking a callback parameter is called once
  bound to one named function and once bound to a different named function
- **THEN** each call site's generated direct call targets that call's own
  bound function's compiled instantiation, with no indirection through a
  shared dispatch point

### Requirement: A generic instantiation's ambient requirements compile to the same hidden-parameter convention as non-generic callables
Where a generic instantiation's body requires an ambient value, the backend
SHALL compile it using the same flattened hidden-parameter convention used
for a non-generic callable with the same requirement, and a sibling
instantiation of the same generic declaration whose callback requires no
ambient value SHALL carry no hidden ambient parameter.

#### Scenario: An ambient-requiring generic instantiation carries hidden parameters
- **WHEN** a generic function calls a callback that reads an ambient value
- **THEN** that instantiation's compiled function signature carries one
  hidden parameter per required ambient slot, in the instance's planned
  field order

#### Scenario: A callback-free sibling instantiation carries no hidden ambient parameter
- **WHEN** the same generic declaration is also called, in the same
  program, bound to a callback that reads no ambient value
- **THEN** that instantiation's compiled function signature carries no
  hidden ambient parameter

### Requirement: A generic instantiation's compiled behavior matches the interpreter, including owned-value final state
For a program compiled to Wasm and run in an independent engine, and the
same program run through the checked-HIR interpreter, every generic
instantiation reached SHALL produce equal return values, equal runtime-error
classification, and equal final state for any owned or moved value the
instantiation's body touches.

#### Scenario: A non-Copy argument moved into a generic consuming callback matches the interpreter
- **WHEN** a program declares `fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(move x) }`
  and calls it with a non-`Copy` argument
- **THEN** the compiled module and the interpreter report the same result,
  and repeated calls through the same generic consuming callback inside a
  bounded loop do not grow the module's memory usage beyond what an
  equivalent single call requires
