## Context

Rhodolite already parses ordinary postfix field access into `ExprKind::Field`, evaluates fields from struct values, and infers declared field types for known non-optional struct receivers. Optional core typing represents `T?` with one bit on `KnownType`, treats `nil` contextually, and types `T? ?? T` as `T`.

The agreed surface form is `receiver.?field`. It is a read-only, short-circuiting projection: an optional receiver containing a struct yields the selected field, while `nil` yields `nil`. Optional assignment and optional method calls are deliberately not part of this change.

Integration exposed a gap in the existing optional core: a function can declare `T?`, but a present `T` cannot currently be passed or returned as that type. The approved correction is a contextual injection at typed destinations. It changes compatibility only when the expected type is `T?`; the expression itself still infers as `T`, and equality still compares exact types.

ADR-0004 requires explicit approval before adding structure. The approved structural addition is exactly one AST variant, `ExprKind::OptionalField(Box<Expr>, String)`. No token kind, runtime value kind, environment, or type representation is added.

## Goals / Non-Goals

**Goals:**

- Parse `receiver.?field` as a postfix field-read expression.
- Evaluate its receiver exactly once and propagate `nil` without field lookup.
- Require a known receiver to have optional struct type `S?`.
- Infer the declared field type with its optional bit set, flattening `T?` to `T?`.
- Support repeated chains such as `user.?profile.?name`.
- Reject ordinary field access on known optional receivers.
- Allow present `T` values at destinations expecting `T?`, while rejecting `T?` at destinations expecting `T`.
- Preserve exact optionality for equality and preserve each expression's inferred type.
- Preserve module rewriting and ambient-requirement traversal through the new expression.

**Non-Goals:**

- Optional field assignment.
- Optional method invocation.
- Backward contextual inference for `nil.?field` or another receiver whose type is unknown.
- Explicit optional constructors, nested optional representation, or changes to `??`.
- Method or associated-function signature resolution.

## Decisions

### 1. Add a distinct `OptionalField` AST variant

Use `ExprKind::OptionalField(Box<Expr>, String)` rather than adding an `optional` flag to `Field`. Existing `Field` also represents a method callee in `Call(Field(...), args)`. Keeping the variants separate makes the approved invariant visible: `OptionalField` is a value-producing read only and is never a call target or assignment target.

The alternative flag would force every ordinary field and method match to inspect a boolean and would make accidental optional method support easier. One explicit traversal arm in each AST consumer is longer but easier to audit.

### 2. Reuse `Dot` followed by `Question`

The lexer already emits `Tok::Dot` and `Tok::Question`, so `.?` needs no combined token. The postfix parser consumes `.` and then optionally `?`; an identifier after `?` produces `OptionalField`, while the existing path without `?` produces `Field`.

After constructing `OptionalField`, an immediately following call suffix is rejected as unsupported optional method invocation. The assignment parser likewise rejects `OptionalField` as the left side of `=`. Rejecting these forms during parsing keeps malformed read-only syntax out of later stages.

### 3. Short-circuit in the evaluator without a new value kind

Evaluate the receiver once. If it is `Value::Nil`, return `Value::Nil`. If it is a struct, reuse the ordinary field lookup behavior. Any other runtime value receives the same class of field-access error as an ordinary read.

Nested AST nodes naturally implement chains: each optional field receives either the previous struct field value or `Nil`. There is no desugaring that could duplicate receiver evaluation.

### 4. Set one optional bit on the declared field type

For a known receiver `S?`, require `S` to be a declared struct and the field to exist. Clone the declared field type and set `optional = true`. This maps both `field: T` and `field: T?` to result `T?`, matching the language's single optional bit and avoiding an unspellable nested optional.

A known non-optional receiver used with `.?` is an error because the operator exists specifically to acknowledge possible `nil`. Conversely, ordinary `.field` on known `S?` becomes an error instructing the user to use `.?`; it no longer silently falls outside inference.

An unknown receiver remains deferred and yields no inferred type. This preserves the incremental checker boundary until arrays and method calls acquire types.

### 5. Traverse the receiver exactly like ordinary field access

Module canonicalization recursively rewrites the receiver. Requirement analysis scans the receiver and records any slot use it contains, but the field name itself creates no call edge or ambient requirement. No new requirement-analysis structure or algorithm is needed.

### 6. Inject present values only at optional destinations

Centralize destination compatibility so an actual known `T` satisfies an expected `T?` when the nominal name matches. This relation applies to direct-call arguments, declared returns, struct-literal fields, field assignments, and established local reassignments. Exact matches remain valid, and an actual `T?` never satisfies an expected non-optional `T`.

Inference is unchanged: a `T` expression remains `T`; the checker does not rewrite its AST or type. Equality continues to require exact nominal type and optionality, so `T == T?` remains an error. Fallback keeps its existing stricter shape `T? ?? T -> T`.

An explicit constructor such as `some(value)` would make presence visible in source, but it would add syntax or a distinguished runtime operation merely to populate already-representable non-`nil` optional values. Contextual injection is the smaller rule and matches the existing contextual treatment of `nil`.

## Risks / Trade-offs

- **[Risk] `.?\n` token joining could differ from ordinary dotted chains.** → Add lexer/parser coverage for same-line and continued postfix forms using the existing `Dot` and `Question` rules.
- **[Risk] The new AST arm could be missed in a traversal.** → Let exhaustive Rust matches identify every consumer, then add module and requirement regression tests.
- **[Risk] Optional method syntax could accidentally parse as field-then-call.** → Reject a call suffix whose callee expression is `OptionalField`.
- **[Risk] Runtime field errors and static diagnostics could drift.** → Reuse ordinary struct lookup behavior in evaluation and mirror existing type-checker field tests.
- **[Risk] Flattening loses whether both receiver and field were absent.** → This is intentional: the language has one optional bit and neither runtime nor type system distinguishes absence provenance.
- **[Risk] Contextual injection could accidentally weaken operators or equality.** → Apply it only through typed-destination compatibility; keep equality and fallback checks explicit and add reverse-direction and equality regressions.

## Migration Plan

1. Add the AST variant, postfix parser, read-only parse guards, evaluator behavior, and traversal arms with focused tests.
2. Add static receiver/field validation, flattened result inference, chaining, and ordinary-access diagnostics.
3. Add contextual `T -> T?` destination compatibility with tests for arguments, returns, struct fields, and assignments, while preserving exact equality.
4. Add CLI integration tests and update the grammar, overview, README, and checker boundary documentation.
5. Run formatting, the full Rust suite, both maintained examples, and OpenSpec validation.

Rollback is a normal change revert; no persisted data or external interface migration is involved.

## Open Questions

None.
