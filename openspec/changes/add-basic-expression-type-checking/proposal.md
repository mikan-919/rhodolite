## Why

The checker still treats literals, field reads, and operators as unknown, so basic mistakes such as returning a string from an integer function or reading a missing field survive until evaluation. These expressions are the next smallest inference boundary to cross before optional values, arrays, and method resolution can be typed coherently.

## What Changes

- Add reserved built-in scalar types `int`, `bool`, `str`, and `unit`.
- Infer scalar literal, non-optional struct-field, arithmetic, equality, and unary-negation result types.
- Check known operands, conditions, assertions, struct field values, field assignments, and local reassignments against their required types.
- **BREAKING**: Normalize built-in type spellings to lowercase and update the canonical program from undeclared `UserId` / `Time` placeholders and uppercase scalar spellings to the reserved built-in names.
- Keep optional values, arrays, method and associated-function resolution, trait/impl conformance, and exhaustive type-name validation outside this change.

## Capabilities

### New Capabilities

- `basic-expression-type-checking`: Define built-in scalar types and statically type-check scalar literals, ordinary struct-field access, primitive operators, boolean conditions, and known assignments.

### Modified Capabilities

None.

## Impact

- `src/typecheck.rs` gains built-in type facts, field lookup, operator rules, and generalized compatibility checks.
- Declaration collection or module checking rejects attempts to redeclare reserved built-in type names.
- Existing tests, examples, and documentation migrate `Int`, `Bool`, `Str`, `UserId`, and `Time` where they denote built-in scalar values.
- The parser and evaluator retain their current expression syntax and runtime operator behavior; no new dependency is required.
