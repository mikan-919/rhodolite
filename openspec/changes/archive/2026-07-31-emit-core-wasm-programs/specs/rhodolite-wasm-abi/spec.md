## Purpose

Defines a small versioned Rhodolite ABI over Core Wasm so frameworks can invoke
compiled programs without making Component Model or host-language conventions
part of the compiler contract.

## ADDED Requirements

### Requirement: ABI v0 separates entry and public function exports
Every generated production module SHALL export the selected `main` instance as
`__rhodolite_main`. Each explicitly selected public function SHALL be exported
under its local public re-export name. Internal declaration names,
fully-qualified names, and specialization names SHALL NOT become exports merely
because their functions are emitted. Export names SHALL be unique, and the
reserved entry name SHALL NOT be usable as a public function export name.

#### Scenario: Entry is exported under the reserved name
- **WHEN** a supported program is built
- **THEN** the module exports its entry function as `__rhodolite_main`

#### Scenario: Aliased public function uses its local name
- **WHEN** the entry module declares `pub use users::{find as find_user}`
- **THEN** the module exports the selected function as `find_user` and not as `find` or its fully-qualified name

#### Scenario: Reserved name collides
- **WHEN** a public re-export would use the name `__rhodolite_main`
- **THEN** the Wasm build fails with a source-positioned reserved-name diagnostic

### Requirement: ABI v0 supports only scalar public signatures
The entry `main` SHALL take no parameters and return `unit`, `bool`, or `int`.
Public function parameters SHALL each be `bool` or `int`, and their return type
SHALL be `unit`, `bool`, or `int`. A production root with any other public
signature, including a `unit` parameter, optional type, string, struct, enum, or
array, SHALL be rejected before emission without restricting those types in
non-public unreachable declarations.

#### Scenario: Supported scalar public signature
- **WHEN** a selected public function takes Boolean and integer parameters and returns an integer
- **THEN** the function can be represented by ABI v0

#### Scenario: Unit return is supported
- **WHEN** a selected public function returns `unit`
- **THEN** its Wasm function has no result value

#### Scenario: Unit parameter is rejected
- **WHEN** a selected public function declares a `unit` parameter
- **THEN** the build reports that ABI v0 cannot represent that public parameter

#### Scenario: Main has a parameter
- **WHEN** `main` declares any parameter
- **THEN** the Wasm build rejects its entry signature

### Requirement: Scalar types have fixed Core Wasm representations
At ABI v0 boundaries, Rhodolite `int` SHALL map to Wasm `i64`, Rhodolite
`bool` SHALL map to Wasm `i32`, and a Rhodolite `unit` return SHALL map to no
Wasm result. A public Boolean argument SHALL accept only `i32` values `0` and
`1`; any other value SHALL trap before the Rhodolite function body executes.
Boolean results SHALL be normalized to `0` or `1`.

#### Scenario: Integer round trip
- **WHEN** a host passes any signed 64-bit bit pattern to an `int` parameter and receives an `int` result
- **THEN** both values cross the boundary as Wasm `i64` with the same bit patterns

#### Scenario: Valid Boolean argument
- **WHEN** a host passes `0` or `1` to a `bool` parameter
- **THEN** the Rhodolite body observes `false` or `true` respectively

#### Scenario: Invalid Boolean argument
- **WHEN** a host passes an `i32` other than `0` or `1` to a `bool` parameter
- **THEN** the exported wrapper traps before invoking the Rhodolite body

#### Scenario: Boolean result is normalized
- **WHEN** a public Rhodolite function returns a Boolean
- **THEN** its Wasm export returns exactly `0` or `1`

### Requirement: Runtime failures cross ABI v0 as traps
Runtime failure reached during an ABI v0 invocation SHALL surface to the host as
a Wasm trap. ABI v0 SHALL NOT add status results, error buffers, exception
objects, or source-position payloads to public signatures. Successful results
SHALL remain indistinguishable from the corresponding Rhodolite return values.

#### Scenario: Division failure reaches the host
- **WHEN** a host invocation reaches division by zero or signed division overflow
- **THEN** the invocation traps rather than returning a scalar result

#### Scenario: Successful invocation
- **WHEN** a public function completes without runtime failure
- **THEN** the host receives only the scalar result prescribed by its declared return type

### Requirement: Modules embed deterministic ABI metadata
Every ABI v0 module SHALL contain exactly one custom section named
`rhodolite.abi`. Its payload SHALL be canonical UTF-8 JSON containing the
numeric ABI version, the reserved entry export and its source-level signature,
and every explicit public function export with its source-level parameter and
return types. The metadata SHALL list public functions by ascending export name
and SHALL distinguish the entry from ordinary public exports. It SHALL contain
no WIT or Component Model contract.

#### Scenario: Host discovers the interface
- **WHEN** a host reads the `rhodolite.abi` custom section
- **THEN** it can distinguish ABI version 0, the entry, and every public function's Rhodolite scalar signature without parsing source code

#### Scenario: Metadata ordering is stable
- **WHEN** source declaration or traversal order does not change the public names or signatures
- **THEN** the custom-section payload remains byte-identical

#### Scenario: Unknown custom section is ignored
- **WHEN** a generic Core Wasm engine does not understand `rhodolite.abi`
- **THEN** it can still validate, instantiate, and invoke the module according to Core Wasm rules
