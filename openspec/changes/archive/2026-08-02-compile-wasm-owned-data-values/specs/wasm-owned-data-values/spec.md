## Purpose

Defines how ownership-checked strings, structs, enums, optionals, arrays, and indirect values execute in generated Core WebAssembly with deterministic storage management and no host runtime dependency.

## ADDED Requirements

### Requirement: Owned data has deterministic Core Wasm representation
The Wasm backend SHALL assign every reachable owned-data type a finite, deterministic linear-memory representation. Struct fields and enum payloads SHALL follow declaration order, optional and enum discriminants SHALL identify the initialized payload, and every declared `indirect` edge SHALL use an address-sized indirection. Equivalent loaded programs SHALL produce the same layouts and Wasm bytes.

#### Scenario: Struct layout is stable
- **WHEN** the same ownership-safe program containing a struct is built twice
- **THEN** its field offsets, size, alignment, and generated module bytes are identical

#### Scenario: Recursive value uses declared indirection
- **WHEN** a finite value of a type such as `Node { indirect next: Node? }` is generated
- **THEN** each present `next` edge refers to a separate finite allocation and `nil` terminates the chain

#### Scenario: Layout cannot fit linear memory
- **WHEN** a reachable type's size, alignment, or element stride cannot be represented safely in 32-bit Core Wasm memory
- **THEN** the build fails with a source-positioned target diagnostic before publishing an artifact

### Requirement: Generated modules manage owned storage without imports
Generated modules that reach owned data SHALL contain their own linear memory and allocator. Allocation SHALL return suitably aligned non-overlapping storage, SHALL reuse storage released by ordinary drop, and SHALL trap on size overflow, failed memory growth, or exhausted address space. The module SHALL require no host allocator, WASI, garbage collector, reference counter, or runtime borrow check.

#### Scenario: Allocation and drop repeat in a bounded loop
- **WHEN** a generated program repeatedly allocates and drops bounded-size owned values in a loop
- **THEN** released storage is reusable rather than requiring memory growth for every iteration

#### Scenario: Allocation cannot complete
- **WHEN** an allocation size overflows or Core Wasm memory cannot grow enough to satisfy it
- **THEN** execution traps without returning an invalid or overlapping allocation

#### Scenario: Module remains import-free
- **WHEN** a program using owned strings, structs, and arrays is built
- **THEN** the resulting Core Wasm module validates without any required host imports

### Requirement: Owned value operations match the reference interpreter
Generated code SHALL preserve source evaluation order and the interpreter's results for string literals; struct and enum construction; ordinary and optional field access; field assignment; optional injection and `nil`; `??`; exhaustive and guarded `match`; array literals; and shared, mutable, and consuming `for` loops. A move SHALL transfer the existing owned value rather than clone it, and a borrow SHALL refer to the checked owner rather than allocate an independent value.

#### Scenario: Struct mutation is visible through a checked borrow
- **WHEN** generated code mutably borrows a struct, assigns one of its fields, and later reads the owner
- **THEN** it observes the assigned value exactly as the interpreter does

#### Scenario: Match executes one arm
- **WHEN** generated code matches an enum with payloads and guards
- **THEN** it evaluates the subject once, tests arms in source order, and executes only the selected arm

#### Scenario: Optional fallback is short-circuited
- **WHEN** generated code coalesces a present optional value with a fallback expression
- **THEN** it produces the present value in the checked ownership mode without evaluating the fallback

#### Scenario: Array iteration respects its ownership mode
- **WHEN** generated code iterates an array by shared borrow, mutable borrow, or move
- **THEN** elements are respectively borrowed, exclusively mutable, or transferred as owned values and the array's post-loop state matches the interpreter

### Requirement: Clone and equality are structural
Generated `clone()` SHALL recursively allocate independent storage for every owned string, direct or indirect field, active enum payload, present optional payload, and array element. Generated equality SHALL compare the same structure through shared reads, SHALL short-circuit on the first difference, and SHALL neither move nor clone its operands.

#### Scenario: Deep clone is independent
- **WHEN** a generated program clones a struct containing a string, an array, and an indirect child and then mutates the clone
- **THEN** the original remains unchanged and both values are dropped independently

#### Scenario: Compound equality preserves owners
- **WHEN** generated code compares two equal arrays of payload enums
- **THEN** equality returns true and both arrays remain initialized and usable

### Requirement: Checked drop plans drive generated cleanup
For every reachable body, generated code SHALL execute the ownership checker's drop plan on normal fallthrough, `return`, branch exit, and loop exit. It SHALL track conditional initialization at runtime where control flow requires it, SHALL skip moved sources, and SHALL recursively release each still-owned buffer or indirect allocation exactly once. A trap SHALL not perform language-level unwinding.

#### Scenario: Early return cleans exited scopes
- **WHEN** generated code returns from a nested scope containing initialized owned locals
- **THEN** those locals are dropped in the order recorded by the checked plan before the function returns

#### Scenario: Moved value is not dropped twice
- **WHEN** one control-flow path moves an owned local before leaving its scope
- **THEN** the generated cleanup skips the source and drops only the surviving owner

#### Scenario: Consuming field projection drops the remainder
- **WHEN** generated code moves an owned field out of a struct
- **THEN** it transfers that field, drops every still-owned remainder recorded by the plan, and does not later drop the consumed struct root
