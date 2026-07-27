## Context

`typecheck.rs` currently indexes struct fields, enum declarations, and enum variants. Its local environment stores `Option<String>`, and `infer` recognizes only typed locals, enum variants, and struct literals. Consequently, a direct function call is traversed but neither checked nor assigned its declared return type.

Module loading already canonicalizes declarations and references before type checking. The checker can therefore index top-level `Item::Fn` signatures by canonical name without introducing a second name-resolution mechanism.

ADR-0004 requires new structures to be declared before implementation. This change introduces the two structures described below; it does not alter the requirement-analysis core.

## Goals / Non-Goals

**Goals:**

- Reject a direct top-level call with the wrong number of arguments.
- Reject argument and return mismatches when both expected and actual types are known.
- Carry a direct call's declared return type into existing local and enum-field checks.
- Preserve deterministic, context-bearing diagnostics and whole-program operation.

**Non-Goals:**

- Inferring types for operators, `nil`, `??`, arrays, fields, branches, or loops.
- Defining subtype or optional unwrapping rules.
- Resolving trait, instance, or associated methods.
- Validating trait declarations against implementations.
- Diagnosing unknown expressions merely because their type cannot yet be inferred.

## Decisions

### 1. Represent known expression types explicitly

Add a small `KnownType { name, optional }` value and use it in the local environment, field declarations, and function signatures. Equality is nominal and includes the optional bit.

This replaces `Option<String>` as the type fact while retaining the outer `Option`: `None` continues to mean “this checker cannot infer the expression,” whereas `KnownType { optional: true, .. }` means “the checker knows this exact optional type.” Reusing a bare string would conflate those two states; reusing the AST `Type` would couple semantic facts to syntax-oriented data.

No new optional expression rule follows from this representation. It only preserves optional annotations across typed identifiers and direct call results.

### 2. Index top-level function signatures with declarations

Extend `Decls` with a canonical function-name map whose values contain owned parameter types and an optional declared return type. All signatures are collected before bodies are checked, so forward and recursive calls work without ordering rules.

Keeping the index in `Decls` reuses the checker's existing whole-program declaration pass. A separate call-resolution pass would duplicate traversal and canonical-name handling.

### 3. Limit call checking to direct top-level functions

Only `Call(Ident(name), args)` whose canonical name is in the function index receives signature checks and a return type. Arity is always checked. Argument compatibility is checked only when `infer` knows the argument type; known types must match nominal name and optionality exactly.

`Path` and `Field` callees remain outside the capability because choosing an applicable impl or trait method needs receiver typing and method-candidate rules. Unknown direct callees remain the responsibility of existing name-resolution diagnostics.

### 4. Check returns at the existing inference boundary

For a function with a declared return type, check every explicit `return value` reachable in its syntax tree and the body's final expression. Emit a mismatch only when `infer` knows the returned expression's type. A bare `return`, an empty body, or an unknown expression produces no new type diagnostic in this change.

The final expression is checked because Rhodolite blocks are value-based. Traversing explicit returns is necessary because they may occur before or inside nested blocks. This change does not attempt control-flow completeness or prove that every path returns.

### 5. Keep diagnostics local and deterministic

Diagnostics identify the containing function, called function or return position, and expected versus actual type. Argument mismatches are emitted in source argument order, while declaration and body traversal retain program order.

This follows the current `Vec<String>` diagnostic API. Source-span rendering remains a later change.

## Risks / Trade-offs

- **[Risk] Partial inference allows some invalid calls or returns through.** → State the inference boundary explicitly and test that unknown expressions do not create false positives.
- **[Risk] Exact optional equality may look like full optional semantics.** → Restrict it to preserving and comparing annotations; keep `nil` and `??` uninferred.
- **[Risk] Return checking can duplicate diagnostics inside nested final blocks.** → Give final-expression checking a single owner per function body and let ordinary expression traversal handle explicit `return` nodes only.
- **[Risk] Method calls remain unchecked while direct calls are checked.** → Diagnose only callees present in the top-level function index and leave method selection to the next dedicated capability.
