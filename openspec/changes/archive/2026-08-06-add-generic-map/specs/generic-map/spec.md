## Purpose

Fixes the observable contract of the generic `Map<T>` trait and its
ordinary-Rhodolite-source `[T]` implementation: what `map<U>` declares, how
`move xs.map(f)` and `xs.clone().map(f)` behave, and how the callback's
ambient requirement and per-element ordering are guaranteed, checked and
executed identically by the interpreter and Core Wasm.

## ADDED Requirements

### Requirement: `Map<T>` is declarable and `[T]` implements it in ordinary Rhodolite source
The language SHALL accept a declaration of `trait Map<T> { fn map<U>(self,
f: fn(T -> U) -> [U]) }` and SHALL accept `impl<T> Map<T> for [T]` as a
generic `impl` whose body is ordinary, type-checked Rhodolite source,
contract-checked against the trait like any other generic `impl`, requiring
no compiler-synthesized body.

#### Scenario: `Map<T>` trait declares its method
- **WHEN** a program declares `trait Map<T> { fn map<U>(self, f: fn(T -> U) -> [U]) }`
- **THEN** the checker accepts the declaration and registers `map` as a
  consuming (`self`) generic trait method with its own type parameter `U`

#### Scenario: `impl<T> Map<T> for [T]` is declared and checked as ordinary source
- **WHEN** a program declares `impl<T> Map<T> for [T] { fn map<U>(self, f: fn(T -> U) -> [U]) { ... } }`
  with a body built from `push`, `for`, and array-literal syntax
- **THEN** the checker type-checks the body once, rigidly, the same way it
  checks any other generic impl method body, and requires no
  compiler-provided implementation

### Requirement: `move xs.map(f)` consumes the array and calls `f` once per element left to right
Calling `map` on an array value SHALL require the `move` receiver modifier,
consuming the array, and SHALL invoke `f` exactly once per element in the
array's order, passing each element to `f` by move, producing a new array
of `f`'s results in the same order.

#### Scenario: Each element is passed to the callback once, in order
- **WHEN** `move xs.map(f)` is called on an array `[a, b, c]`
- **THEN** `f` is called exactly three times, in the order `f(a)`, `f(b)`,
  `f(c)`, and the result is `[f(a), f(b), f(c)]`

#### Scenario: Mapping an empty array calls the callback zero times
- **WHEN** `move xs.map(f)` is called on an empty array
- **THEN** `f` is never called and the result is an empty array of `f`'s
  result type

#### Scenario: A non-`Copy` element is moved into the callback
- **WHEN** `move xs.map(f)` is called on an array of a non-`Copy` element type
- **THEN** each element is moved into its call to `f`, and the source
  array (having been moved as the receiver) is no longer usable

#### Scenario: `move` is required at the call site
- **WHEN** `xs.map(f)` is called without the `move` receiver modifier on a
  bound array local
- **THEN** checking reports the missing consuming-receiver modifier, the
  same way any other consuming method call without `move` is reported

### Requirement: No borrowed `map` exists; `xs.clone().map(f)` is the way to keep the original array
The language SHALL NOT provide a shared- or mutable-receiver `map`. A
caller that wants to keep its original array after mapping SHALL clone it
first.

#### Scenario: `xs.clone().map(f)` leaves the original array usable
- **WHEN** a program calls `xs.clone().map(f)` and later uses `xs`
- **THEN** checking accepts the later use of `xs`, and `xs.clone().map(f)`'s
  result is independent of `xs`

#### Scenario: A borrowed receiver is rejected
- **WHEN** a program calls `&xs.map(f)` or `&mut xs.map(f)`
- **THEN** checking rejects the call the same way any other receiver-mode
  mismatch against a consuming method is rejected

### Requirement: The callback's ambient requirement propagates through `map` and trait dispatch
Where `f` requires an ambient slot, that requirement SHALL be inferred as a
requirement of `xs.map(f)`'s call site and, transitively, of the function
containing that call site, unless it provides the slot around the call —
the same propagation an indirectly-called or generic-forwarded callback
already gets.

#### Scenario: A callback's ambient requirement reaches the caller of `map`
- **WHEN** a callback passed to `move xs.map(f)` calls an ambient slot
- **THEN** the caller of `xs.map(f)` is inferred to require that slot
  unless it provides it around the call

#### Scenario: A callback with no ambient requirement stays clean
- **WHEN** a callback passed to `move xs.map(f)` calls no ambient slot
- **THEN** `xs.map(f)`'s call site acquires no ambient requirement

#### Scenario: A missing provider through `map` reports a path naming `map`
- **WHEN** a production root reaches a `map` call whose callback requires
  an ambient slot with no matching `with` provision anywhere on the path
- **THEN** compilation fails before Wasm generation with a diagnostic
  whose reachability path names the `map` call

### Requirement: Interpreter and Core Wasm execute `map` with matching results
For a program that calls `map`, the checked-HIR interpreter and the
compiled Core Wasm module, run in an independent engine, SHALL produce the
same resulting array (element sequence, length, and element values) and
the same final ownership state of any moved element, for a `Copy` element
type, a non-`Copy` element type, and an empty array.

#### Scenario: Same result for a `Copy` element type
- **WHEN** a program calls `move xs.map(f)` on an array of a `Copy` element
  type and returns or inspects the result
- **THEN** the interpreter and the compiled module report the same
  resulting array

#### Scenario: Same result and ownership state for a non-`Copy` element type
- **WHEN** a program calls `move xs.map(f)` on an array of a non-`Copy`
  element type
- **THEN** the interpreter and the compiled module agree on the resulting
  array's contents and on each mapped value's final ownership state

#### Scenario: Same result for an empty array
- **WHEN** a program calls `move xs.map(f)` on an empty array
- **THEN** the interpreter and the compiled module both report an empty
  result array
