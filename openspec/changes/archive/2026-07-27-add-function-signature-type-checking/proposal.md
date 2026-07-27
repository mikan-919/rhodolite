## Why

The checker currently stops type information at function-call boundaries, so declared parameter and return types do not reject known mismatches and call results cannot inform later checks. Function signatures are the smallest next boundary to cross before adding operator, collection, or method typing.

## What Changes

- Index top-level function signatures during type checking.
- Check direct top-level calls for argument count and known argument-type mismatches.
- Infer a direct call's declared return type for use by later checks.
- Check known explicit and final return values against a function's declared return type.
- Keep operator rules, optional-value semantics, array element typing, and method resolution outside this change.

## Capabilities

### New Capabilities

- `function-signature-type-checking`: Type-check direct top-level function calls and function return values when the relevant expression types are known.

### Modified Capabilities

None.

## Impact

- `src/typecheck.rs` gains signature indexing, richer known-type propagation, and call/return diagnostics.
- Type-checker unit tests and CLI integration tests gain accepted and rejected function-call examples.
- No parser, evaluator, dependency, or public CLI contract changes are required.
