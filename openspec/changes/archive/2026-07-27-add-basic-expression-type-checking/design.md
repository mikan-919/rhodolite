## Context

`typecheck.rs` currently represents exact nominal facts as `KnownType { name, optional }`, but `infer` only recognizes typed locals, enum variants, struct literals, and declared results of direct top-level calls. Scalar literals, field reads, unary expressions, and binary expressions therefore lose their types. Existing signature and return checks intentionally skip those unknown expressions, and field-value checking is limited to known enum mismatches.

The evaluator already establishes the runtime behavior relevant to this change: integer-only arithmetic and negation, equality returning a boolean, boolean-only conditions and assertions, and field lookup through runtime struct values. The checker can make those rules static without changing evaluation.

The canonical program currently relies on undeclared nominal spellings (`UserId` and `Time`) accepting integer values only because literals are unknown. It also mixes lowercase `bool` and `unit` with uppercase scalar spellings elsewhere. Introducing literal types requires one canonical spelling for each built-in scalar.

## Goals / Non-Goals

**Goals:**

- Make `int`, `bool`, `str`, and `unit` reserved, non-overridable built-in type names.
- Infer exact types for scalar literals, ordinary non-optional field reads, arithmetic, equality, and negation.
- Reject invalid operands, non-boolean conditions and assertions, invalid field reads, and known assignment mismatches before evaluation.
- Feed the new facts into existing local binding, direct-call argument, and function-return checks.
- Preserve deterministic whole-program diagnostics and the current runtime semantics.

**Non-Goals:**

- Defining `T?`, `Some` / `None`, optional chaining (`a.?b`), or fallback (`a ?? b`).
- Inferring array element types or checking loop binders from arrays.
- Resolving method or associated-function signatures.
- Checking trait declarations against impls.
- Rejecting every undeclared type annotation.
- Eliminating the checker's temporary unknown state for expression forms outside this change.
- Adding source-span rendering or changing the evaluator.

## Decisions

### 1. Use lowercase reserved names for built-in scalar types

The built-in type identities are exactly `int`, `bool`, `str`, and `unit`. Scalar literals infer as `int`, `bool`, and `str`; `unit` remains the type name for expressions whose established runtime result is unit.

Loaded declarations may not claim a built-in type name. The check belongs in the existing declaration/checking pipeline rather than in operator-specific code, so no user-defined declaration can change the meaning of a literal. These are semantic built-in type names; the parser does not need new expression syntax.

The canonical program and test fixtures migrate uppercase scalar names and the undeclared integer stand-ins `UserId` and `Time` to `int`. Adding aliases, newtypes, or context-polymorphic integer literals would enlarge the language merely to preserve placeholder names and is rejected for this change.

### 2. Extend the existing exact `KnownType` inference boundary

Keep nominal equality as exact name plus optionality. Add inference cases for:

- integer, boolean, and string literals;
- `Field(receiver, name)` when the receiver has a known non-optional struct type and the field exists;
- unary negation after validating an `int` operand;
- arithmetic after validating two `int` operands;
- equality, whose result is always `bool`.

The ordinary recursive `infer` path makes chained non-optional reads such as `user.profile.name` work without a separate propagation pass. Existing `let`, direct-call argument, and return checks consume these facts automatically.

Optional field access is deliberately absent. A known optional receiver does not silently unwrap. Its eventual behavior belongs to the dedicated optional change alongside `Some` / `None`, `a.?b`, and `??`.

### 3. Separate result inference from operand diagnostics

Expression traversal owns diagnostics, while `infer` remains a fact query. The checker validates operands and field existence during `check_expr`, then `infer` computes the result type for downstream consumers.

This preserves the current architecture in which inference can be called repeatedly without duplicating diagnostics. Combining diagnostics with inference would make a field or operator nested inside a call susceptible to repeated errors whenever multiple checks ask for its type.

For equality, the result is `bool` even when one operand remains temporarily unknown. When both operand types are known, they must be exactly equal. Arithmetic and negation report an error only for a known non-`int` operand; unsupported unknown subexpressions remain deferred.

### 4. Generalize known-value compatibility checks

Replace the enum-only field compatibility helper with one exact known-type comparison used by:

- every known struct-literal field value;
- every known field-assignment value;
- reassignment to an ordinary binding whose type was established by its parameter annotation or initializer.

A binding with a temporarily unknown initializer stays unknown; later assignments do not establish a flow-sensitive type. Inferring from a later assignment would require merging branches and proving the assignment executes, which is outside this change.

The generalized rule subsumes the existing cross-enum rejection and additionally catches scalar, struct, and optional-bit mismatches whenever both sides are known.

### 5. Check boolean-only contexts at the same boundary

`if`, `elif`, and `while` conditions and `assert` operands must be `bool` whenever their expression type is known. Unknown expressions from unsupported forms remain deferred.

This is a use-site constraint rather than a new truthiness model. Rhodolite continues to have no implicit conversion from integers, strings, structs, or enums to booleans.

### 6. Keep unsupported expressions explicitly temporary

The language-level destination is that every valid expression has a type and no successful check contains an unresolved type. During incremental implementation, method calls, associated calls, arrays, and optional operations may still return no `KnownType`; this change SHALL NOT turn that implementation state into a user-visible wildcard type.

Documentation must distinguish “not implemented by this checker stage” from a language feature. Each later capability narrows this set until it disappears.

## Risks / Trade-offs

- **[Risk] Lowercase built-ins break existing examples and fixtures.** → Migrate the repository atomically and run the full suite plus both CLI examples.
- **[Risk] Repeated inference can accidentally repeat diagnostics.** → Keep `infer` side-effect free and emit all errors from the single expression traversal.
- **[Risk] Generalizing enum checks can expose many previously hidden mismatches at once.** → Add focused unit and CLI tests for each use site, then update only fixtures whose old validity depended on unknown literal types.
- **[Risk] Temporary unknown expressions make the checker sound only within its documented boundary.** → State exclusions in the capability and overview, and do not add wildcard compatibility or flow-sensitive guesses.
- **[Risk] Reserved names could be enforced inconsistently across declaration kinds or modules.** → Centralize the check at the declaration-collection/name-validation boundary and test each representative declaration path, including imported modules.

## Migration Plan

1. Reserve the four built-in type names and add literal type facts.
2. Migrate repository source, examples, tests, and documentation from scalar placeholders and uppercase spellings.
3. Add field and operator inference plus operand and boolean-context checks.
4. Generalize field-value and reassignment compatibility.
5. Run unit tests, CLI integration tests, `cargo run -- examples/canonical.rd`, and the missing-handler example.

Rollback is a single change revert: no persisted data or external interface migration is involved.

## Open Questions

None. Optional representation and operations are intentionally deferred to a dedicated change.
