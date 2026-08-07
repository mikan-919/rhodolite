# wasm-traits-and-ambient

## Purpose

Defines how checked methods, trait implementations, ambient slots, and scoped provider selection execute in generated import-free Core WebAssembly modules.

## Requirements

### Requirement: Reachable methods compile with their checked receiver semantics
The Wasm backend SHALL lower reachable inherent methods, associated functions, and trait implementation methods using their statically resolved callable and checked `self`, `&self`, or `&mut self` receiver mode. It SHALL preserve receiver-before-argument evaluation, argument evaluation order, mutation visibility, ownership transfer, return behavior, and deterministic destruction.

#### Scenario: Inherent method mutates its receiver
- **WHEN** reachable code invokes an `&mut self` inherent method on an owned struct
- **THEN** the generated module applies the same mutation observed by the reference interpreter

#### Scenario: Consuming method transfers ownership
- **WHEN** reachable code invokes a `self` method with an ownership-safe receiver
- **THEN** the generated module transfers and destroys the value according to the checked ownership plan without a double drop

#### Scenario: Associated function has no receiver
- **WHEN** reachable code invokes a statically resolved associated function
- **THEN** the generated call passes only its declared arguments and produces the same result as the reference interpreter

### Requirement: Trait and slot calls use the statically selected implementation
Every reachable trait implementation method SHALL be emitted as an ordinary specialized callable instance. A slot call SHALL target the implementation selected by its enclosing provider context directly; generated code SHALL NOT perform runtime implementation lookup, vtable dispatch, or a switch over implementation identifiers.

#### Scenario: Value slot call receives the provider
- **WHEN** `with db(provider)` encloses a reachable `db.save(value)` call
- **THEN** the generated call directly invokes the selected implementation method with that provider as its receiver

#### Scenario: Type slot call has no runtime provider
- **WHEN** `with clock<Frozen>` encloses a reachable `clock::zero()` call
- **THEN** the generated call directly invokes the selected implementation function without carrying a provider value

#### Scenario: Different implementations specialize the same body
- **WHEN** the same transitive caller is reached under two different implementations of one slot
- **THEN** the module contains distinct specialized caller instances whose slot calls directly target their respective implementations

### Requirement: With provisions preserve scoped provider semantics
Generated `with` expressions SHALL evaluate all value provisions once, in source order, under the outer provider context before applying any provision from that same `with`. The body SHALL execute under a copied context with all named slots replaced together. A nested `with` SHALL shadow only its named slots and SHALL restore the outer selections after its body completes.

#### Scenario: Provisions observe the outer context
- **WHEN** a multi-provision `with` has a later value expression that uses a slot also replaced by that `with`
- **THEN** the later expression observes the outer provider rather than an earlier provision in the same head

#### Scenario: Nested provision restores the outer provider
- **WHEN** an inner `with clock(inner)` shadows `with clock(outer)` and execution later continues in the outer body
- **THEN** calls inside the inner body use `inner` and subsequent outer-body calls use `outer`

#### Scenario: Provision expression is evaluated once
- **WHEN** a value provision expression has an observable mutation and its slot is used more than once in the body
- **THEN** the expression is evaluated once and every use refers to the same provided instance

### Requirement: Ambient records carry only required provider handles
A specialized callable instance SHALL receive one hidden provider handle for each value-level slot in its inferred ambient requirement, ordered deterministically by slot identity. It SHALL receive no handle for type-only requirements or unrelated slots. Calls SHALL project the callee's required handles from incoming or locally provided handles without cloning the provider value or creating shared ownership.

#### Scenario: Ambient-free callable has no hidden values
- **WHEN** a reachable callable has no value-level ambient requirement
- **THEN** its generated internal signature contains no ambient provider handle

#### Scenario: Transitive caller forwards only the callee requirements
- **WHEN** a function does not mention a slot directly but transitively calls code that requires it
- **THEN** its specialized instance receives and forwards the required provider handle without a source-level parameter

#### Scenario: Unrelated provision does not widen a callee
- **WHEN** a caller provides two slots but a reachable callee requires only one
- **THEN** the callee's generated instance receives only the required slot handle

### Requirement: Provider handles obey the checked ownership plan
Generated value provisions and slot receivers SHALL preserve the statically checked shared, mutable, moved, or temporary provider mode. A provider handle SHALL identify the original storage rather than an implicit clone. Temporaries and moved providers owned by a provision scope SHALL be destroyed exactly once when that scope exits normally; borrowed providers SHALL remain owned by their source.

#### Scenario: Shared provider remains available
- **WHEN** a bound provider is supplied by shared access and the `with` body completes
- **THEN** mutations are not introduced and the original owner remains usable afterward

#### Scenario: Mutable provider exposes mutation
- **WHEN** a bound provider is supplied with `&mut` and a slot method mutates it
- **THEN** the mutation is visible through the original owner after the `with` body

#### Scenario: Temporary provider is cleaned up
- **WHEN** a fresh owned provider expression is supplied to a `with`
- **THEN** the generated module retains one stable handle throughout the body and destroys the provider once on normal scope exit

### Requirement: Canonical ambient programs build reproducibly
The canonical production program SHALL build as import-free Core Wasm, validate in an independent engine, and execute to the same observable result as the reference interpreter. Repeated builds from identical inputs SHALL remain byte-identical.

#### Scenario: Canonical production path executes
- **WHEN** `examples/canonical.rd` is built for Wasm and its entry export is invoked
- **THEN** the module validates and produces the canonical successful result using its production providers

#### Scenario: Provider substitution requires no forwarding edits
- **WHEN** an equivalent maintained fixture changes only the implementations supplied by its root `with`
- **THEN** generated transitive calls use the replacements without source changes to intermediate functions

#### Scenario: Ambient build is repeated
- **WHEN** the same trait-and-ambient program is built twice with the same compiler and options
- **THEN** the generated byte sequences are identical
