## ADDED Requirements

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
