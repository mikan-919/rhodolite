## Why

Optional core typing can now construct, compare, and unwrap `T?`, but there is no way to safely project a field through an optional struct. Adding the smallest read-only projection closes that gap without pulling method resolution or optional mutation into the same change.

## What Changes

- Add postfix optional field syntax `receiver.?field`.
- Evaluate the receiver exactly once; return `nil` without reading a field when it is `nil`.
- Require a known receiver to have optional struct type `S?` and require `S` to declare the selected field.
- Type `S?.?field` as the declared field type with one optional bit, flattening an already optional field from `T?` to `T?`.
- Reject ordinary `receiver.field` when the receiver has known optional type instead of silently deferring it.
- Support chained reads such as `user.?profile.?name`.
- Allow a known non-optional `T` to satisfy an expected `T?` at typed destinations such as arguments, returns, struct fields, and assignments. Keep expression inference unchanged and keep `T` and `T?` unequal for equality.
- Keep optional field assignment and optional method calls outside this change.

## Capabilities

### New Capabilities

- `optional-field-access`: Syntax, evaluation, typing, diagnostics, chaining, and read-only boundaries for `receiver.?field`.

### Modified Capabilities

- `basic-expression-type-checking`: Remove optional field access from the expression forms that this checker explicitly defers, reject ordinary field access on known optional receivers, and allow `T` values at `T?` assignment destinations.
- `function-signature-type-checking`: Allow `T` values at `T?` parameter and return destinations while continuing to reject the reverse direction.
- `optional-core-type-checking`: Define contextual `T -> T?` injection without changing inferred expression types or exact equality.

## Impact

The change affects tokenization and parsing, the expression AST, module and requirement traversals, evaluation, destination compatibility checks, CLI integration tests, and language documentation. It adds no dependency and does not change storage, method resolution, runtime values, equality semantics, or the existing `T? ?? T -> T` rule.
