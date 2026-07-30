## Context

See `proposal.md` for motivation. `module::load` currently merges loaded modules into one owned AST and rewrites visible names to canonical strings. `typecheck::check` builds private declaration indexes, walks every body, computes `Outcome::Typed`, `Diverges`, or `Poisoned`, and then discards those facts. `requirement::analyze` and `eval::Interp::new` each rebuild their own indexes from the AST and resolve calls and slots again by string.

The closed type-checking contract now guarantees that successful checking has one concrete type for every value-producing expression and one selected declaration for every call. The HIR must preserve those facts once, retain source spans for diagnostics, and be owned independently of the AST so it can later feed the C backend.

## Goals / Non-Goals

**Goals:**

- Establish one owned, typed, resolved representation as the boundary after loading and checking.
- Make declaration, field, variant, slot, local, implementation, and callable references structural rather than string-based.
- Make requirement analysis consume explicit call edges and slot uses instead of rediscovering them from syntax.
- Make the reference interpreter execute resolved operations without member-candidate or source-name lookup.
- Preserve all existing observable behavior, diagnostic text and spans, requirement paths, declaration order, and test output.
- Leave a representation that the next ambient ABI and C-lowering changes can consume without consulting the AST.

**Non-Goals:**

- Changing source syntax, type rules, name resolution, effect inference, or runtime value semantics.
- Adding a control-flow graph, SSA, monomorphization, closure conversion, or C-specific layout.
- Defining the ambient C ABI.
- Persisting IDs across builds or making them part of a public serialization format.
- Optimizing expression storage or evaluator performance.
- Keeping two permanent interpreters.

## Decisions

### 1. HIR is an owned program with program-local typed IDs

Add `src/hir.rs` containing an owned `hir::Program` and small newtype IDs. IDs are dense indexes, allocated in deterministic declaration order, and are meaningful only with the `Program` that created them.

```rust
struct FunctionId(u32);
struct CallableId(u32);
struct StructId(u32);
struct FieldId(u32);
struct EnumId(u32);
struct VariantId(u32);
struct TraitId(u32);
struct TraitMethodId(u32);
struct TraitImplId(u32);
struct SlotId(u32);
struct TestId(u32);
struct LocalId(u32);
struct ExprId(u32);
```

Each arena entry retains its canonical display name and declaration span. Diagnostics and user-facing requirement rendering use those names; semantic lookup uses IDs.

Using strings inside otherwise typed HIR was rejected because it would preserve the duplicate lookup and ambiguity risk this change removes. Globally stable or hashed IDs were rejected because the program is whole-program and no cache or serialized artifact consumes HIR yet.

### 2. Value types distinguish builtins and nominal declarations

HIR owns a public internal type representation equivalent to:

```text
Type {
    kind:
        Builtin(Int | Bool | Str | Unit)
        Struct(StructId)
        Enum(EnumId)
        Array(Type)
    optional: bool
}
```

Trait names and slots are not value types. Optional remains one bit on the completed type, matching the current `[T]?` versus `[T?]` rules. Control-flow divergence is represented separately as `ExprResult::Diverges`, not as a user-visible type.

`typecheck::KnownType` becomes transitional and is removed once all checks emit `hir::Type` directly. Reusing AST `Type` was rejected because it contains canonical strings and would let later phases accidentally repeat nominal resolution.

### 3. Callables and declarations use separate IDs

Top-level functions, inherent methods, and trait implementation methods all have executable bodies and are stored in one callable arena. A callable records:

- its canonical display name;
- owner kind (top-level function, inherent implementation, or trait implementation);
- receiver presence;
- parameter locals and types;
- effective return type;
- body expression IDs;
- declaration span.

Source-level declaration arenas retain structure needed for display and dispatch. A `TraitImplId` identifies one `impl Trait for Struct` and maps every `TraitMethodId` in that trait to the implementing `CallableId`.

Trait declaration methods have signatures but no executable body, so `TraitMethodId` is not interchangeable with `CallableId`. This distinction is required for slot dispatch: the source call resolves statically to a trait method while the surrounding `with` selects the implementing callable.

Flattening every declaration into one universal ID was rejected because it would replace string confusion with kind checks. Keeping methods nested only inside AST-like declarations was rejected because direct call edges and runtime dispatch need compact executable identities.

### 4. Expressions live in an arena and carry result classification and span

Each callable and test has a local expression arena or indexes a program expression arena. The chosen implementation may use either ownership layout, but every `ExprId` must be unambiguous within its documented owner and every HIR expression must contain:

```text
Expr {
    result: Value(Type) | Diverges
    kind: resolved HirExprKind
    span: Span
}
```

Blocks, heads, match arms, arguments, and nested expressions refer to `ExprId`. The representation stays structured rather than becoming a CFG so it remains close to the language semantics and easy for the reference interpreter to execute.

An annotated AST was rejected because later stages could still inspect unresolved names and because AST nodes represent source distinctions that a backend should not need. A CFG was deferred because it would combine semantic resolution with a second lowering problem.

### 5. Lexical references are resolved to unique locals and members

Every parameter, `self`, `let`, loop binding, and match payload binding receives a `LocalId` unique within its callable or test. HIR identifiers become one of the valid resolved value forms, principally:

- `Local(LocalId)`;
- zero-field `Struct(StructId)`;
- fieldless `Variant(VariantId)`.

HIR field reads and assignments carry `FieldId`. Struct literals carry `StructId` and field/value pairs keyed by `FieldId`. Enum constructors and match patterns carry `VariantId`; payload bindings carry `LocalId`.

Slots are not ordinary value identifiers. Slot use is encoded only in resolved slot calls and provisions. A source identifier that cannot be classified this way already fails type checking and therefore cannot enter HIR.

Local IDs preserve current lexical behavior: ordinary blocks do not introduce a scope, match arms do, and `with` introduces slot shadowing for its body. Runtime environments may initially remain maps keyed by `LocalId`; changing them to indexed frames is a later optimization.

### 6. Calls are represented by their already selected operation

HIR distinguishes:

```text
Direct {
    callable: CallableId,
    args
}
ConcreteMethod {
    callable: CallableId,
    receiver,
    args
}
ConcreteAssociated {
    callable: CallableId,
    args
}
SlotMethod {
    slot: SlotId,
    method: TraitMethodId,
    receiver_kind: Value | Type,
    args
}
EnumConstructor {
    variant: VariantId,
    args
}
```

The exact enum names may be simplified during implementation, but no successful HIR call may require lookup by source method name. Receiver syntax validity and argument compatibility have already been checked.

For `with slot(value)` and `with slot<Type>`, HIR provision nodes carry `SlotId`, the concrete `TraitImplId`, and the value expression when present. At runtime, a slot call selects the implementing `CallableId` from that provision's trait-method map. This retains dynamic lexical provider selection while removing method-name lookup.

Storing only the statically chosen implementing callable on a slot call was rejected because the same function can execute under different `with` providers. Keeping `slot + method-name` was rejected because it would leave runtime member resolution in place.

### 7. Type checking and HIR construction are one semantic traversal

The public boundary becomes conceptually:

```rust
typecheck::check_and_lower(&ast::Program)
    -> Result<hir::Program, Vec<Diag>>
```

Implementation occurs in two passes:

1. declaration pass assigns all declaration IDs, canonical names, signatures, fields, variants, slots, implementations, and callable shells;
2. body pass uses the existing bidirectional checking logic to allocate locals and resolved expressions while it has the necessary type and target facts.

If any diagnostic exists, partial HIR is discarded. A successful result cannot contain `Poisoned`, unknown types, or unresolved targets. Test helpers may keep a compatibility `check` wrapper temporarily, but CLI and final module APIs use the result-returning boundary.

Running the existing checker and then performing a second “HIR recovery” traversal was rejected because it would repeat inference and call resolution and could disagree with the validation pass. Mutating AST nodes during checking was rejected because it would couple source and semantic representations.

### 8. Requirement analysis uses resolved body identities

Requirement analysis keeps its existing fixed-point algorithm and public diagnostics but changes its input facts:

- direct and concrete calls target `CallableId`;
- slot calls target a virtual body identity derived from `TraitMethodId`;
- implementation method facts contribute both to their concrete `CallableId` and to the matching trait-method virtual identity;
- direct slot accesses use `SlotId`;
- call-site provisions use `SlotId → SlotLevel`;
- names are looked up only when rendering paths and summaries.

This mirrors the current dual keys `impl Trait::method` and `impl Type::method` without synthesizing those keys as strings. The public `Analysis` may expose names as before, while its internal maps use body and slot IDs.

Changing the fixed-point algorithm or adding SCC optimization was rejected as unrelated. Specializing requirements by concrete provider was deferred to the future monomorphization work.

### 9. The HIR interpreter stores runtime identities, not source names

The HIR interpreter executes `ExprId` and resolved operation variants. Runtime compound values use `StructId` and `VariantId` rather than canonical strings. Ambient bindings hold:

```text
Type { implementation: TraitImplId }
Value { implementation: TraitImplId, value: Value }
```

Concrete calls jump directly to `CallableId`. Slot calls read the current ambient binding and select the callable by `TraitMethodId` from its `TraitImplId`.

Because values no longer own display names, rendering moves from `Value::show()` to a program-aware helper such as `Interp::show(&Value)` or `Program::show_value(&Value)`. User-facing formatting remains byte-for-byte compatible.

The existing runtime checks remain only where they validate runtime state rather than repeat source resolution. Candidate search by method name, struct-name lookup, variant-name lookup, and provision trait-name checking are removed from the final HIR execution path.

### 10. Migration uses parallel implementations followed by one cutover

Add HIR lowering first while the AST interpreter and AST requirement analyzer remain authoritative. Then:

1. compare lowered HIR structure against focused snapshots;
2. add HIR requirement analysis and compare rendered requirements and diagnostics on the maintained corpus;
3. add HIR evaluation and compare values, output, mutation, and failures against the AST interpreter;
4. switch CLI to HIR;
5. remove AST execution and AST requirement scanning after the full suite passes.

Temporary modules may be named `hir_eval` and `hir_requirement`; after cutover they replace or become the existing `eval` and `requirement` modules so the final architecture has one implementation of each behavior.

A flag-driven permanent dual pipeline was rejected because it doubles maintenance. A one-shot rewrite was rejected because it would remove the reference behavior before equivalence is demonstrated.

### 11. Equivalence is the acceptance criterion

This change has no delta specs. Its behavioral contract is the union of current main specs. Verification includes:

- all existing parser, loader, checker, requirement, evaluator, and CLI tests;
- HIR structural tests for every accepted expression and declaration form;
- side-by-side requirement render and unsatisfied-path comparisons;
- side-by-side evaluation of the canonical program and focused mutation, optional, match, provider, recursion, and error cases;
- unchanged source spans and CLI output for pre-execution and runtime failures.

No test is removed merely because it exercised the old representation; it is migrated to the new boundary or retained as a frontend test.

## Risks / Trade-offs

- [The HIR model grows into a second AST] → Include only resolved semantics required by analysis, evaluation, and the next lowering; omit source-only syntax distinctions.
- [A second checker traversal disagrees with the first] → Generate HIR during the authoritative type-checking traversal and discard partial output on diagnostics.
- [Program-local IDs make diagnostics unreadable] → Store canonical display names and declaration spans in arenas and translate IDs only at presentation boundaries.
- [Slot dispatch is accidentally specialized to one provider] → Represent slot calls by `SlotId + TraitMethodId` and provider bindings by `TraitImplId`.
- [Requirement paths change order or names] → Preserve declaration order explicitly and compare complete rendered output and related diagnostic spans before cutover.
- [Runtime value rendering loses names] → Make rendering program-aware and pin existing CLI strings with tests.
- [Parallel implementations drift during migration] → Keep the overlap short, add differential tests first, and remove the AST pipeline in the same change after cutover.
- [One large refactor becomes impossible to bisect] → Commit declaration IDs, expression lowering, requirement migration, evaluation migration, and cutover as separately verified stable snapshots.
- [Dense IDs overflow or are mixed across kinds] → Use distinct newtypes, checked conversion at allocation, and debug assertions at arena access.

## Migration Plan

1. Add HIR IDs, types, declaration arenas, expression arena, and deterministic debug rendering without changing the current pipeline.
2. Refactor declaration collection to assign IDs and build HIR declaration shells while preserving checker diagnostics.
3. Generate resolved HIR expressions and locals during successful type checking; add full expression-form coverage.
4. Add HIR requirement analysis and differential tests, then switch requirement consumers.
5. Add the HIR interpreter and differential tests, then switch CLI execution.
6. Remove AST requirement/evaluation code and temporary compatibility APIs.
7. Update architecture documentation and mark the HIR phase complete in the compiler roadmap.

Each stable step runs formatting, the complete test suite, and strict OpenSpec validation before being committed. Rollback is commit-by-commit; no persisted user data or public artifact format is involved.
