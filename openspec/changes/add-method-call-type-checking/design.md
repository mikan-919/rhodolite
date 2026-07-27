## Context

`src/typecheck.rs` currently indexes struct fields, enum variants, and top-level function signatures. Its call branch deliberately handles only `Call(Ident, ...)`; `Call(Field, ...)` and `Call(Path, ...)` are walked but remain untyped. The interpreter already resolves these forms from concrete types, ambient slot traits, and `impl` candidates, so invalid calls can survive checking and fail only during evaluation.

The recently completed array work gives the canonical `InMemoryDb.users` loop a concrete element type. Method resolution is now the remaining boundary that prevents `db.find(id) ?? return false`, `clock.now()`, and constructors from carrying declared types through the whole canonical program.

This change must preserve whole-program, nominal checking; the existing module loader has already canonicalized declaration and type names. It must also preserve the language distinction between normal locals, which can shadow slot names, and ambient slot bindings introduced by `with`.

## Goals / Non-Goals

**Goals:**

- Validate trait `impl` declarations against trait contracts before checking their bodies.
- Resolve value methods, ambient slot methods, concrete associated functions, and slot-associated functions with the interpreter's existing selection semantics.
- Reuse existing arity, directional compatibility, expected-type propagation, and result inference behavior for every resolved signature.
- Make all currently used call forms in `examples/canonical.rd` statically typed.

**Non-Goals:**

- Generic methods, overload ranking, implicit conversions, extension methods, or dynamic dispatch.
- Ownership, borrowing, aliasing, or array mutation guarantees.
- Static validation that every `with` provision value implements the slot trait; this change validates declared implementations and calls, while provision checking remains a separate boundary.
- Enum methods, optional method invocation, or first-class function/method values.
- Changing parser syntax, evaluator behavior, module resolution, or diagnostics to carry spans.

## Decisions

### 1. Extend the declaration index with contracts, slots, and implementation candidates

Add static signature records for trait methods and `impl` methods alongside the current top-level `FnSig`. Index trait declarations by trait and member name, slots by slot and trait name, and implementation candidates by concrete type and member name. Each implementation candidate retains its optional trait name and whether the signature declares `self`.

The signature representation will contain explicit parameters and the optional return type; `self` remains a separate boolean and is never counted among call arguments. This matches the AST and prevents receiver handling from leaking into ordinary arity and argument loops.

Alternative considered: infer calls by repeatedly scanning `Program.items`. A single declaration index is preferred because the checker already uses this architecture for functions and fields, it handles forward references naturally, and it makes ambiguity checks deterministic.

### 2. Validate trait implementations as a declaration pass

During index construction, validate that each trait `impl` names a declared trait and struct, contains each contract member exactly once, contains no extra members, and matches receiver form, parameter types, parameter count, and optional return type. Parameter names are excluded from identity because they are local implementation bindings rather than contract types.

Inherent `impl` blocks have no external contract, but duplicate member declarations remain separate candidates and therefore become ambiguous at call sites. Their bodies continue to be checked against their own declared signatures.

Alternative considered: accept partial or extra trait implementations and diagnose only when a member is called. That would make conformance depend on reachability and allow an invalid declared handler to pass, so declaration-time validation is required.

### 3. Represent local values and ambient slots distinctly

Replace the current `Locals` value payload with a binding record that distinguishes a normal value carrying `Option<KnownType>` from an ambient slot carrying its declared trait. Top-level slot names resolve as slots only when no local shadows them; entering `with` installs a slot binding in the inner scope, while provision expressions are still checked in the outer scope.

This preserves the evaluator's lexical rule for `with db(db)` and lets call resolution distinguish an unknown-typed local from a statically known slot instead of representing both as `None`.

Alternative considered: identify every matching identifier from the global slot table. That would incorrectly resolve locally shadowed names as ambient calls.

### 4. Resolve calls by syntax before applying one shared signature checker

Call resolution produces either a selected static signature, a diagnostic, or a deferred result:

- `value.member(args)` first checks whether `value` is an ambient slot. If so, it selects the slot trait declaration. Otherwise, an inferable concrete receiver type selects candidates from that type's inherent and trait implementations.
- `Type::member(args)` selects candidates registered for the resolved concrete type.
- `slot::member(args)` selects the slot trait declaration when the first path component is an unshadowed slot.
- A receiver whose type is unknown defers selection, preserving the current incremental inference boundary.

Concrete lookup mirrors the interpreter: select all candidates with the member name, report absence, require uniqueness when no slot trait disambiguates, then validate dot versus path syntax against `has_self`. Slot lookup is contract-based because its runtime concrete type intentionally varies.

After resolution, one helper distributes parameter expectations into argument expressions, checks explicit arity and known argument types with the existing destination compatibility rule, and returns the signature's declared result type. Direct top-level calls use the same helper where practical so call-form behavior cannot drift.

Alternative considered: resolve dot and path calls with separate checking implementations. A common selected-signature path is preferred because receiver selection is the only semantic difference; arity, argument compatibility, and result inference are identical.

### 5. Keep static and runtime selection aligned through behavioral tests

The evaluator remains unchanged. Type-checker tests will cover the same distinguishing cases as evaluator tests: missing members, ambiguous same-named trait members, slot-trait disambiguation, and receiver-form errors. End-to-end canonical tests verify that newly inferred results flow through optional fallback, field access, assignment, and returns without changing execution.

Extracting a shared resolver between the checker and interpreter is not part of this change because their inputs differ: static resolution works on declared types and contracts, while runtime resolution works on values and provided concrete implementations. Shared scenario tests provide a smaller, clearer synchronization boundary.

## Risks / Trade-offs

- **[Risk] Static and runtime candidate rules may drift because they use separate indexes.** → Mirror existing evaluator cases in type-checker tests and keep the resolution order explicit in one static helper.
- **[Risk] Refactoring local bindings to distinguish slots may regress lexical shadowing.** → Add tests for a local shadowing a slot and for `with db(db)`, checking provision values outside and bodies inside the new slot binding.
- **[Risk] Newly inferred call results will expose errors in repository examples that were previously deferred.** → Run the full test suite and every maintained example, fixing only genuine signature inconsistencies rather than weakening inference.
- **[Risk] Exact trait conformance may reveal currently accepted malformed `impl` declarations.** → Add focused diagnostics and migrate repository-owned fixtures in the same change; no external compatibility promise exists yet.
- **[Trade-off] Provision compatibility remains a runtime check.** → Record it as the next narrow type-checking boundary instead of coupling value-to-trait proof with call resolution.
