## Why

`T?` annotations already exist and the evaluator already carries `nil` and short-circuiting `??`, but the type checker treats both expressions as unknown. This lets invalid optional returns, fallbacks, arguments, and assignments pass checking, and leaves the first item in the post-v1 type-checking roadmap unfinished.

## What Changes

- Give `nil` contextual compatibility with optional destinations while keeping an uncontextualized `nil` binding unresolved.
- Type fallback as `T? ?? T -> T`, including a right-hand side that exits through `return`.
- Reject a known non-optional left operand and a known incompatible fallback value.
- Feed fallback result types into the existing exact argument, assignment, equality, and return checks.
- Keep optional field access/chaining outside this change; its surface syntax and propagation rules remain a separate decision.
- Update the maintained type-system documentation and add unit and CLI coverage for the new boundary.

## Capabilities

### New Capabilities

- `optional-core-type-checking`: Contextual `nil` checking and the core type rule, diagnostics, and short-circuit contract for `??`.

### Modified Capabilities

- `basic-expression-type-checking`: Remove `nil` and `??` from the expression forms that this checker explicitly defers.

## Impact

The change affects `src/typecheck.rs`, its unit tests, CLI integration tests, and the type-system boundary documented in `README.md`, `docs/overview.md`, and `docs/grammar.md`. It does not change parsing, runtime representation, evaluation order, dependencies, or optional field syntax.
