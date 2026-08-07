## ADDED Requirements

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
