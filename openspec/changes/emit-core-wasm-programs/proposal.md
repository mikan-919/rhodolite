## Why

Rhodolite has closed static checking, typed HIR, and a deterministic ambient
specialization plan, but its only execution path is still the interpreter. The
next vertical slice should produce a stable, host-neutral artifact without
binding the language or compiler to the still-evolving WebAssembly Component
Model.

## What Changes

- Add `pub use` as the language's ordinary public re-export mechanism; an entry
  module's explicitly selected public functions define its host-callable
  surface.
- Add `rhodolite build <entry.rd> --target wasm [-o <path>]`, preserving the
  existing positional interpreter command.
- Emit deterministic Core WebAssembly modules for reachable scalar functions
  and basic control flow, using the ambient specialization plan from the first
  emitter version.
- Define Rhodolite Wasm ABI v0: a reserved `__rhodolite_main` entry export,
  flat function exports named after their local `pub use` names, scalar
  lowering rules, strict Boolean boundary validation, traps for runtime
  failure, and an embedded `rhodolite.abi` JSON custom section.
- Define `int` as signed 64-bit two's-complement arithmetic with wrapping
  addition, subtraction, multiplication, and negation, so the interpreter and
  generated Wasm have one execution meaning.
- Separate production specialization roots (`main` plus explicit exports) from
  test roots. Reject only unsupported constructs reachable from the selected
  production roots; retain the existing test planning capability.
- Defer Component Model/WIT packaging, rich public ABI types, runtime transport
  for non-empty ambient records, data layout, host capabilities, Wasm-native
  test execution, and detailed runtime-error transport.

## Capabilities

### New Capabilities

- `public-reexports`: Public module re-exports, aliases, collisions, and the
  explicit function surface selected by the entry module.
- `integer-runtime-semantics`: Backend-independent signed 64-bit integer
  overflow and division behavior.
- `core-wasm-build`: CLI behavior, reachable scalar lowering, specialization
  roots, deterministic output, unsupported-feature diagnostics, and executable
  validation for Core Wasm builds.
- `rhodolite-wasm-abi`: ABI v0 export names, scalar call conventions, Boolean
  validation, traps, and embedded deterministic interface metadata.

### Modified Capabilities

None.

## Impact

- Parser, AST, module loader, HIR, and diagnostics gain public re-export
  information while preserving existing `use` behavior.
- Requirement analysis and `ambient_abi::Plan` gain caller-selected root sets
  without losing the current combined main-and-test plan.
- A new Core Wasm lowering/emission path and build CLI are added; the
  interpreter remains the reference implementation.
- Integer evaluation is made explicit and build-mode-independent.
- The project gains focused Wasm encoding, validation, metadata, and execution
  test support. No Component Model or WIT dependency becomes part of the
  language contract.
