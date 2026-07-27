## Why

The checker still drops all type information at method and associated-function calls, so the canonical program's `db.find(id) ?? ...`, `clock.now()`, and constructors can bypass arity, argument, return, and downstream expression checks. Arrays are now typed, making call resolution the next smallest boundary to cross and the highest-priority gap in the post-v1 roadmap.

## What Changes

- Index trait declarations and inherent and trait `impl` methods for static call resolution.
- Validate trait implementations against their declared contracts, including method set, receiver form, parameters, and return type.
- Resolve calls through concrete values, ambient slots, and type paths using the same candidate-selection rules as the interpreter.
- Check resolved calls for receiver form, ambiguity, arity, and known argument types, and propagate declared return types into later checks.
- Keep ownership, mutation/aliasing rules, generic dispatch, and enum methods outside this change.

## Capabilities

### New Capabilities

- `method-call-type-checking`: Static contract validation and signature checking for trait implementations, value/slot method calls, and associated-function calls.

### Modified Capabilities

- `function-signature-type-checking`: Narrow its temporary unsupported-call boundary now that method and associated-function signatures are covered by a dedicated capability.
- `basic-expression-type-checking`: Remove method and associated-function calls from the expressions whose types are intentionally deferred.

## Impact

- `src/typecheck.rs` gains method/trait indexes, implementation validation, call candidate selection, diagnostics, and result inference.
- Static checking of `examples/canonical.rd` becomes end-to-end across all currently supported call forms.
- `docs/overview.md`, `docs/grammar.md`, and related type-checker documentation must describe the new guarantee and the next remaining gaps.
- No syntax, runtime behavior, external dependency, or public CLI change is required.
