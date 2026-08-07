# rhodolite-wasm-abi

## Purpose

Defines a small versioned Rhodolite ABI over Core Wasm so frameworks can invoke
compiled programs without making Component Model or host-language conventions
part of the compiler contract.

## Requirements

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

### Requirement: ABI version follows the selected public value surface
A generated module whose entry and public functions all satisfy ABI v0 SHALL continue to use ABI version `0` and its existing scalar signatures. If any selected public parameter or result is an owned string, struct, payload enum, optional, or array, the module SHALL use ABI version `1` for its entire public surface. `main` SHALL still take no parameters, borrowed public signatures and `unit` parameters SHALL remain rejected, and owned values that contain borrowed fields SHALL remain unavailable.

#### Scenario: Scalar module remains ABI v0
- **WHEN** every selected public signature contains only ABI v0 scalar types
- **THEN** the module metadata reports version `0` and its public Core Wasm signatures remain unchanged

#### Scenario: One rich export selects ABI v1
- **WHEN** one selected public function accepts or returns an owned data type
- **THEN** the module metadata reports version `1` and every selected export follows ABI v1

#### Scenario: Borrowed rich value is rejected
- **WHEN** a selected public function exposes `&T` or `&mut T`
- **THEN** the build fails before emission with a source-positioned signature diagnostic

### Requirement: Callable-typed public signatures are rejected pending Wasm codegen support
The Wasm backend SHALL reject, with a source-positioned diagnostic, a public
function whose parameter or return type is a callable type, stating that the
Wasm target cannot yet represent it. The backend SHALL NOT treat a
callable-typed public parameter or return value as ordinary owned data. This
rejection SHALL apply even though `callable-public-boundary` accepts the same
signature at the type-checking layer, until a later change adds the
handle-based invoke export that signature requires.

#### Scenario: Callable public parameter is rejected by the Wasm backend
- **WHEN** a public function accepted by type checking declares a
  callable-typed parameter
- **THEN** `rhodolite build --target wasm` fails before code generation with
  a diagnostic naming the parameter and stating the Wasm backend cannot yet
  represent it

#### Scenario: Callable public return value is rejected by the Wasm backend
- **WHEN** a public function accepted by type checking declares a
  callable-typed return value
- **THEN** `rhodolite build --target wasm` fails before code generation with
  a diagnostic naming the function and stating the Wasm backend cannot yet
  represent it

### Requirement: ABI v1 transports rich values through canonical bytes
ABI v1 SHALL transport each owned-data parameter as a Core Wasm `(i32 pointer, i32 length)` pair into exported linear memory and each owned-data result as an `(i32 pointer, i32 length)` pair returned from the export. Scalars in mixed signatures SHALL retain their ABI v0 Core Wasm representations. The canonical byte encoding SHALL use little-endian integers, UTF-8 strings, declaration-order struct fields, declaration-order numeric variant tags followed by the active payload, an explicit empty/present optional tag, and a length followed by declaration-typed array elements. The encoding SHALL be independent of the compiler's internal heap layout.

#### Scenario: Rich parameter is decoded by value
- **WHEN** a host passes canonical bytes for an owned struct parameter
- **THEN** the wrapper decodes a fresh Rhodolite-owned value before executing the function body and later host writes cannot alias it

#### Scenario: Rich result is encoded by value
- **WHEN** a public function returns an owned array
- **THEN** the wrapper returns a pointer and length for its canonical encoding and releases the internal returned owner after encoding

#### Scenario: Nested encoding is deterministic
- **WHEN** a value contains nested structs, optionals, payload enums, strings, and arrays
- **THEN** repeated encodings of structurally equal values produce byte-identical sequences

### Requirement: ABI v1 exposes a bounded exchange area
Every ABI v1 module SHALL export its linear memory as `memory` and SHALL export a reserved function `__rhodolite_abi_reserve` that accepts a non-negative byte length and returns an `i32` pointer to a writable exchange area of at least that length. Rich input slices SHALL lie within the most recently reserved exchange area. A rich result slice SHALL remain readable until the next call to `__rhodolite_abi_reserve` or any Rhodolite function export. The host SHALL copy a result before that invalidating call. These reserved exports SHALL NOT expose source-language allocation or manual-free operations.

#### Scenario: Host stages rich arguments
- **WHEN** a host reserves enough exchange bytes, writes one or more canonical argument slices, and invokes an ABI v1 export with slices inside that area
- **THEN** the wrapper can decode those arguments without a host import

#### Scenario: Result lifetime is bounded
- **WHEN** a host receives a rich result and invokes another Rhodolite export before copying it
- **THEN** the previous result slice is no longer guaranteed to contain that result

#### Scenario: Reserved name collides
- **WHEN** a public re-export would use `memory` or `__rhodolite_abi_reserve`
- **THEN** the Wasm build fails with a source-positioned reserved-name diagnostic

### Requirement: ABI v1 validates canonical inputs before execution
An ABI v1 wrapper SHALL validate every rich input slice before the Rhodolite function body executes. It SHALL trap on an out-of-range slice, arithmetic overflow, trailing or truncated bytes, a Boolean other than `0` or `1`, invalid UTF-8, an unknown enum or optional tag, or any length whose decoded value would exceed the slice or available memory. Failed decoding SHALL not expose a partially initialized Rhodolite value to the body.

#### Scenario: Invalid UTF-8 input
- **WHEN** a rich argument encodes a string with invalid UTF-8 bytes
- **THEN** the wrapper traps before the Rhodolite body executes

#### Scenario: Unknown variant tag
- **WHEN** a rich argument contains an enum tag outside its declaration's variants
- **THEN** the wrapper traps before the Rhodolite body executes

#### Scenario: Trailing input bytes
- **WHEN** a rich argument contains a valid encoded value followed by extra bytes
- **THEN** the wrapper traps rather than accepting a non-canonical encoding

### Requirement: ABI v1 metadata describes the wire schema deterministically
An ABI v1 module's single `rhodolite.abi` custom section SHALL retain the existing entry and export descriptions and SHALL add a canonical type table sufficient to encode and decode every public owned-data type without source code. The table SHALL distinguish builtins, optionals, arrays, structs with declaration-order fields, and enums with declaration-order variants and payloads. Recursive references SHALL use deterministic type identifiers. Only types reachable from selected public signatures SHALL appear, and all object keys and type entries SHALL use one documented deterministic order.

#### Scenario: Host discovers a rich signature
- **WHEN** a host reads ABI v1 metadata for a public function using a recursive struct
- **THEN** it can discover the function's flattened Core Wasm signature and canonical recursive wire schema without parsing Rhodolite source

#### Scenario: Private type is omitted
- **WHEN** a loaded owned-data type is not reachable from any selected public signature
- **THEN** it does not appear in the ABI type table solely because internal generated code uses it

#### Scenario: ABI v1 metadata is stable
- **WHEN** source traversal order changes without changing public names or reachable public type declarations
- **THEN** the canonical metadata bytes remain identical
