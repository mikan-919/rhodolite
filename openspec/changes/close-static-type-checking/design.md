## Context

See `proposal.md` for motivation. The current checker represents both “not yet inferred” and “this expression does not produce a value” as `None`. It synthesizes types with `infer`, separately walks expressions for diagnostics, and deliberately skips compatibility checks when either side is unknown. Call resolution similarly distinguishes a resolved call, an invalid call, and an unresolved receiver through nested `Result<Option<_>>`.

That shape was useful while type checking grew one expression family at a time, but it cannot establish the invariant required by a typed HIR. The change also crosses the parser, AST, module canonicalization, requirement scan, evaluator, checker, CLI tests, and language documentation because local type annotations add syntax.

## Goals / Non-Goals

**Goals:**

- Make successful checking mean that every accepted expression is concretely typed or known not to produce a value.
- Make successful checking mean that every call has one statically selected signature.
- Preserve precise source diagnostics without producing a cascade for one root error.
- Supply expected types from destinations and local annotations to contextual expressions.
- Keep the current evaluator as a behavioral oracle and runtime defense while preparing for HIR.

**Non-Goals:**

- Introducing HIR or persisting resolved IDs in the AST.
- General Hindley–Milner inference, unification variables, or inference from later assignments.
- Inferring public function signatures or omitted non-`unit` return types.
- Adding generics, function values, overloading, subtyping, union types, or a user-visible bottom type.
- Removing evaluator checks that become unreachable for statically checked programs.
- Proving that every control-flow path returns; this change checks values on paths that do return.

## Decisions

### 1. An omitted return annotation means `unit`

Every function, trait member, and implementation method has an effective return type. An explicit annotation supplies it; otherwise it is `unit`.

This keeps signatures local and deterministic and matches the current canonical use of omitted annotations for procedures. Inferring a return type from the body was rejected because recursive and mutually recursive functions would require a new constraint solver, and because it would make a declaration’s callable signature depend on body traversal.

Existing declarations that omit an annotation but finish with a non-`unit` value must add an explicit return type or make the final value intentionally `unit`.

### 2. Add `let name: Type = value` as the only local type-ascription form

`ExprKind::Let` gains `annotation: Option<Type>`. Parsing reuses the existing type grammar after `:`, module loading canonicalizes named leaves recursively, requirement analysis ignores the annotation, and evaluation continues to bind only the evaluated value.

The annotation provides an expected type to the initializer and fixes the binding type for later references and assignments. Unannotated bindings still synthesize their type solely from the initializer.

Inferring a bare `nil` or `[]` from a later assignment was rejected because it introduces flow-sensitive backward inference. A general expression-ascription syntax was rejected because this change only needs a type at the binding boundary.

### 3. Replace optional inference with a three-way checking result

Expression checking uses an internal result equivalent to:

```text
Typed(T)     expression produces a value of concrete type T
Diverges     expression exits the current control flow and produces no value
Poisoned     a diagnostic for this expression or a dependency already exists
```

`Diverges` is used for `return` and for composite expressions whose selected path cannot produce a value. It is not a user-visible type and is compatible with any expected result position because control does not continue through that path.

`Poisoned` prevents secondary diagnostics. A parent receiving `Poisoned` does not invent a type and does not emit a generic unknown diagnostic when the child has already explained the failure. Overall checking still fails because the diagnostic set is non-empty.

At the end of a body, any unresolved internal state without an associated diagnostic is a checker invariant violation in tests and must become a positioned user diagnostic in production.

Using `unit` for `return` was rejected because it would incorrectly reject `return false` inside a branch expected to produce another type. Keeping `Option<KnownType>` was rejected because it cannot distinguish divergence, an earlier error, and an unsupported expression.

### 4. Use bidirectional synthesis and checking

The checker exposes two conceptual operations:

```text
synth(expr, env) -> Typed | Diverges | Poisoned
check(expr, expected, env) -> Typed(expected) | Diverges | Poisoned
```

Synthesis handles literals, resolved references, declared call results, field reads, operators, and non-empty homogeneous arrays. Checking passes an expected type into contextual constructs:

- `nil`
- empty and contextual array literals
- annotated binding initializers
- arguments
- returns
- struct fields
- assignments
- match arms
- conditional branches

This does not attempt general bidirectional polymorphic inference; it only replaces the current scattered `mismatch` and `require` behavior with one directional compatibility boundary.

### 5. All accepted expression variants receive an explicit rule

The checker’s exhaustive match over `ExprKind` must classify every variant:

- literals, identifiers, paths, fields, calls, arrays, structs, unary and binary expressions synthesize values;
- `let`, assignment, and `assert` synthesize `unit`;
- `return` diverges after checking its optional value against the effective function return;
- an empty block is `unit`, otherwise its last continuing expression determines its value;
- loops produce `unit`;
- `with` produces its body type;
- `if`/`elif`/`else` joins continuing branches, while a conditional without an `else` produces `unit`;
- `match` joins continuing arm types; an exhaustive empty-enum match with no arms diverges.

Expressions before the final expression of a block are still allowed to produce and discard a value. Total typing does not introduce the future discarded-value warning.

### 6. Call resolution must return a selected signature or a diagnostic

Call resolution may use the expression checker to determine a receiver type, but a successful check cannot retain the current unresolved `None` outcome. The cases are:

- exactly one candidate: use its effective signature;
- no candidate: positioned missing-member diagnostic;
- multiple candidates: positioned ambiguity diagnostic;
- poisoned receiver: propagate `Poisoned`;
- otherwise untyped receiver: positioned cannot-determine-type diagnostic.

Direct functions and member signatures use effective `unit` returns, so every successful call synthesizes a concrete result type.

Persisting `FnId` or `MethodId` is deferred to the HIR change. This change establishes resolution as a checked invariant but need not redesign AST ownership.

### 7. Contextual `nil` must resolve to a concrete optional type

`nil` checked against `T?` is treated as `T?` for that check. Without an expected optional type it is not independently typable. Consequently:

- `let x = nil` fails;
- `let x: User? = nil` succeeds;
- `nil == optional_user` succeeds by using the other operand;
- `nil == nil` fails because neither operand establishes a nominal type;
- `nil ?? value_of_T` succeeds by contextualizing the left as `T?`.

Treating all `nil` values as a universal optional type was rejected because it would reintroduce a wildcard into successful checking.

### 8. Runtime defenses remain during this change

The evaluator continues checking invalid method lookup, non-boolean conditions, invalid provisions, and malformed runtime values. These paths protect direct evaluator unit tests and guard against checker defects. CLI execution must nevertheless demonstrate that statically checked source cannot reach them through a type uncertainty.

Removing the defenses is deferred until execution consumes typed HIR.

## Risks / Trade-offs

- [The change touches every expression family and may produce duplicate diagnostics] → Introduce `Poisoned` before converting deferred cases, and add one root-cause test for every contextual boundary.
- [Changing omitted returns to `unit` may break many existing test fixtures] → Migrate fixtures mechanically and commit that stable migration separately from inference changes.
- [Bidirectional checking may accidentally change optional injection or array invariance] → Preserve current compatibility helpers and retain all positive and negative existing tests.
- [Recursive call resolution could recurse through expression inference] → Resolve signatures from declaration indexes only; never infer a callee’s body to determine its return.
- [A missing rule can silently recreate Unknown] → Add a checker audit that visits every loaded body and asserts no successful expression result remains unclassified.
- [`nil == nil` becomes a breaking error] → Document the required typed binding or comparison with a known optional operand.
- [Keeping runtime defenses duplicates logic temporarily] → Treat the checker as authoritative for CLI programs and remove duplication only after HIR owns resolved types and targets.

## Migration Plan

1. Add parser, AST, module-resolution, evaluator, and scan support for optional local annotations without changing existing inference behavior.
2. Define effective `unit` returns and migrate existing sources and fixtures that relied on an unknown omitted return.
3. Introduce `Typed`, `Diverges`, and `Poisoned` internally while preserving all current diagnostics.
4. Convert expression families to total checking from leaves upward: bindings and returns, operators and fields, arrays and optionals, matches and heads, calls and provisions.
5. Add whole-program audit tests proving that unused loaded declarations are also closed.
6. Update overview and grammar documentation after all tests pass.

Each step must leave the full test suite green and be committed as a stable snapshot. Rollback is by reverting the latest stable commit; no persisted user data or external migration is involved.
