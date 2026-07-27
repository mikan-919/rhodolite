## Context

The parser represents `T?` as the optional bit on `Type`, `nil` as its own expression, and `??` as `BinOp::Coalesce`. The evaluator already implements `nil` as a distinguished runtime value and evaluates `??` left-to-right with a short-circuit: the right side runs only when the left side is `nil`.

The checker represents a known ordinary or optional type as `KnownType { name, optional }`, but deliberately returns no fact for `nil` and `??`. Its compatibility checks are currently split across direct calls, returns, field values, assignments, and equality. Optional core typing needs contextual `nil` behavior at all of those sites without pretending that bare `nil` has a self-contained nominal type.

## Goals / Non-Goals

**Goals:**

- Check `nil` against contextual expected types and accept it only where an optional value is expected.
- Type and validate `T? ?? T` without changing its existing short-circuit evaluation.
- Allow a `return` expression as the fallback branch because it does not produce a value on the continuing path.
- Make the resulting `T` fact available to existing downstream checks.
- Diagnose contextual `nil` misuse deterministically at every existing compatibility site.

**Non-Goals:**

- Adding `Some` / `None` constructors or changing the runtime representation.
- Adding or choosing syntax for optional field access/chaining.
- Flattening nested optionals; the current syntax has one optional bit and cannot spell `T??`.
- Resolving method or associated-function signatures.
- Adding full control-flow or never-type analysis beyond recognizing a direct `return` expression.
- Giving a bare `let x = nil` a flow-sensitive or inferred nominal type.

## Decisions

### 1. Keep `nil` contextual instead of inventing a nominal `nil` type

`nil` is compatible with an expected `T?` and incompatible with an expected non-optional `T`. A bare `nil` still yields no `KnownType`, so `let x = nil` remains unresolved and later assignment does not establish its type.

Compatibility will be centralized in one helper used by calls, returns, struct fields, assignments, and equality. This avoids giving `nil` a wildcard identity and prevents the existing use sites from drifting apart.

An alternative was to represent `nil` as a new `KnownType`. That would either make it compare equal to unrelated optional types or require special cases at every consumer, so it is rejected.

### 2. Define fallback as `T? ?? T -> T`

When the left operand has known type `T?`, the right operand must have exact type `T`; the result is non-optional `T`. A known non-optional left operand is rejected. When the left operand is literal `nil` and the right operand has known non-optional type `T`, the right side supplies the context and the result is `T`.

If the left operand's type is still outside the current inference boundary, fallback checking and result inference remain deferred. This is important for `db.find(id) ?? ...` until method signatures are resolved by the next roadmap item.

Allowing `T? ?? T? -> T?` was considered, but it would make `??` a general optional merge rather than an unwrap-with-fallback operation and would not advance callers toward a usable `T`. The narrower rule is easier to explain and matches the canonical use.

### 3. Treat a direct `return` fallback as a non-continuing branch

For `value ?? return fallback`, the `return` expression is checked against the enclosing function's declared return type, but it does not need to match `value`'s inner type because execution does not continue from that branch. The coalesce result is therefore the inner `T` from the left operand.

This is a local rule for `ExprKind::Return`, not a general `never` type. Blocks, conditionals, and other expressions that happen to return on all paths remain outside this change.

### 4. Give `nil` contextual equality behavior

`nil == value` and `value == nil` are accepted when the other operand has a known optional type, rejected when it has a known non-optional type, and deferred when the other operand is unknown. `nil == nil` is accepted and still has result type `bool`.

Without this rule, removing `nil` from the explicitly unsupported boundary would leave a common contextual use silently unchecked. Equality does not infer a nominal type for a standalone `nil`.

### 5. Preserve the diagnostic/inference split

`check_expr` remains responsible for all diagnostics. `infer` stays side-effect free and gains only result facts for valid-shape coalesce expressions. Contextual compatibility helpers may inspect syntax such as `nil` and `return`, but never emit from `infer`, preventing duplicate diagnostics when downstream checks query a type repeatedly.

## Risks / Trade-offs

- **[Risk] Method-call results still hide the canonical `db.find(...) ?? ...` type from the checker.** → Document that this change completes optional core rules only when the left type is known; method resolution remains the next roadmap step.
- **[Risk] A direct-return exception could grow into ad hoc control-flow typing.** → Restrict it to `ExprKind::Return` and defer general divergence to a future control-flow design.
- **[Risk] Contextual `nil` checks could differ between use sites.** → Route all expected-type checks through one compatibility helper and cover each consumer with tests.
- **[Risk] Existing fixtures intentionally relying on silent `nil` mismatches will fail.** → Update only fixtures whose expected behavior changes, then run the full suite and both CLI examples.

## Migration Plan

1. Add focused failing tests for contextual `nil`, equality, fallback operands, fallback results, and direct-return fallback.
2. Centralize contextual compatibility and implement the new checks and inference.
3. Add CLI integration coverage and update the documented inference boundary.
4. Run formatting, the full Rust test suite, and both maintained examples.

Rollback is a normal change revert; there is no persisted data or external dependency migration.

## Open Questions

None. Optional field syntax and propagation are explicitly deferred to a later change.
