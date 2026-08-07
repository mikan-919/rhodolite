## Purpose

Fixes the minimal, ordinary-source-reachable way to grow an array after
construction: a `Push<T>` trait with a compiler-synthesized `[T]`
implementation, its `xs.push(y)` call-site and ownership contract,
doubling capacity growth, and OOM-traps-via-`unreachable` on a failed
grow, checked and executed identically by the interpreter and Core Wasm.

## Requirements

### Requirement: `Push<T>` is declarable and `[T]` implements it as a compiler builtin
The language SHALL accept a declaration of `trait Push<T> { fn push(&mut
self, x: T) }`. The checker SHALL register `impl<T> Push<T> for [T]` as
satisfying that trait for every element type without requiring or accepting
a parsed body for that implementation's `push` method, and SHALL apply the
same generic-impl contract checking (signature, receiver mode, requirement
inference, ownership planning) to it as it would to a parsed generic impl
with a matching signature.

#### Scenario: `Push<T>` trait declares its method
- **WHEN** a program declares `trait Push<T> { fn push(&mut self, x: T) }`
- **THEN** the checker accepts the declaration and registers `push` as a
  `&mut self` generic trait method taking one argument of the trait's type
  parameter

#### Scenario: `[T]` satisfies `Push<T>` without a parsed impl body
- **WHEN** a program calls `.push(...)` on a value of array type `[T]`
- **THEN** the checker resolves the call to the compiler-synthesized
  `impl<T> Push<T> for [T]`, requiring no user-written `impl` block for it
  to exist

#### Scenario: `push` requires no ambient value and adds no requirement
- **WHEN** requirement inference analyzes a function that calls `xs.push(y)`
- **THEN** that call contributes no ambient slot requirement to the caller

### Requirement: `xs.push(y)` borrows the receiver exclusively without a call-site modifier
A call to the compiler-builtin `Push<T>::push` method on a bound array
value SHALL exclusively borrow that array for the call without requiring
the `&mut` receiver modifier `method-call-type-checking` otherwise requires
for a bound mutable receiver. The pushed argument `y` SHALL be evaluated
and consumed under the same move-once-argument semantics as an ordinary
owned function-call argument.

#### Scenario: No `&mut` modifier is required at the call site
- **WHEN** source calls `xs.push(y)` on a bound, mutably accessible array
  local, with no `&mut` written before `xs`
- **THEN** the checker accepts the call and resolves it to an exclusive
  borrow of `xs`

#### Scenario: A non-exclusively-accessible receiver is rejected
- **WHEN** `xs.push(y)` is called while another borrow of `xs` is live, or
  while `xs` is not mutably accessible
- **THEN** checking fails identifying the conflicting or insufficient
  access to `xs`, the same way any other exclusive-borrow conflict is
  reported

#### Scenario: The pushed value moves once
- **WHEN** a bound non-`Copy` local `y` is passed to `xs.push(y)`
- **THEN** `y` is moved into the array and any later use of `y` is rejected,
  matching an ordinary owned call argument

### Requirement: Push grows capacity by doubling
Pushing onto an array whose length equals its capacity SHALL grow the
array's capacity before appending. Capacity 0 SHALL grow to capacity 1 on
first push. A non-zero capacity SHALL double. Pushing onto an array with
spare capacity SHALL append without reallocating.

#### Scenario: First push from empty allocates capacity one
- **WHEN** `push` is called on an array with length 0 and capacity 0
- **THEN** the array's capacity becomes 1 and the pushed element is its
  only element

#### Scenario: Capacity doubles when full
- **WHEN** `push` is called on an array whose length equals its capacity
  (capacity > 0)
- **THEN** the array's new capacity is twice its prior capacity and all
  prior elements remain in order, followed by the pushed element

#### Scenario: Push with spare capacity does not grow
- **WHEN** `push` is called on an array whose length is less than its
  capacity
- **THEN** the array's capacity is unchanged and the pushed element is
  appended after the prior last element

### Requirement: A failed capacity grow traps instead of returning a failure value
Where growing an array's capacity requires allocating new backing storage
and that allocation fails, execution SHALL trap unconditionally (the
existing `unreachable` allocator-failure contract), and `push` SHALL
introduce no source-visible failure value, `Optional` result, or new error
classification of its own.

#### Scenario: Allocation failure during growth traps
- **WHEN** a `push` call needs to grow capacity and the allocator cannot
  satisfy the request
- **THEN** execution traps unconditionally with no recoverable result
  returned to the caller

#### Scenario: `push`'s return type carries no failure case
- **WHEN** the checker computes `push`'s result type
- **THEN** the result type is `unit`, with no optional or enum wrapper for
  an allocation-failure case

### Requirement: Interpreter and Core Wasm execute `push` with matching results
For a program that calls `push` one or more times, the checked-HIR
interpreter and the compiled Core Wasm module, run in an independent
engine, SHALL produce the same array length, the same element values and
order, and the same final ownership state for any element pushed.

#### Scenario: Same content and length after repeated pushes
- **WHEN** a program pushes several elements onto an array, including at
  least one capacity-growth boundary, and returns or inspects the result
- **THEN** the interpreter and the compiled module report the same
  element sequence and length

#### Scenario: Owned element final state matches after a move-in push
- **WHEN** a non-`Copy` value is moved into an array via `push`
- **THEN** the interpreter and the compiled module agree on the pushed
  value's final ownership state (owned by the array, not independently
  reachable from its original binding)
