## Context

See `proposal.md` for motivation and `specs/generic-type-parameters/spec.md`
for the contract. Today `Sig` (`src/ast.rs`) has no type parameter list,
`Item::Trait` has none either, and `Item::Impl` stores its trait reference and
target as bare `String`s (`trait_name: Option<String>`, `type_name: String`),
which cannot spell `impl<T> Map<T> for [T]`. `Tok::Less`/`Tok::Greater` already
exist for `with slot<Type>` and are otherwise unused (`BinOp` has no `<`/`>`),
so they are free for a second, unambiguous use at fixed declaration
positions. Module resolution (`module::resolve_type`) rewrites every
`TypeKind::Named` leaf to a canonical module-qualified name or passes it
through unchanged if unknown; type checking (`typecheck::report_unknown`)
is the single place that rejects a name with no matching declaration. `hir`'s
`Type`/`TypeKind` is exhaustively matched by ownership checking, requirement
analysis, the interpreter, and both Wasm backends, all of which assume every
type is fully concrete.

## Goals / Non-Goals

**Goals:**
- Parse type parameter lists on `fn`, `trait`, and `impl` per MAP-Q1, and
  widen `Item::Impl`'s trait reference and target so `impl<T> Map<T> for [T]`
  is representable.
- Give each declared type parameter a stable identifier valid only inside its
  own declaration, with span-reported duplicate and out-of-scope diagnostics.
- Add a signature representation that can hold type parameter references, used
  only for generic declarations, without adding a type-parameter variant to
  the concrete `hir::Type` that every existing pass already matches
  exhaustively.
- Keep every non-generic declaration's parse, resolution, type-check, dump,
  and execution behavior byte-identical.

**Non-Goals:**
- Type-checking a generic body against its own type parameters as rigid
  variables, inferring call-site type arguments, monomorphization, trait
  resolution across type parameters, and connecting generics to ownership,
  requirement analysis, the interpreter, or Wasm generation. All of that is
  MAP-020 through MAP-070.
- Explicit type-argument syntax at call sites (MAP-Q2), generic `struct`/
  `enum` (MAP-Q1), and constrained type parameters (ROADMAP Non-goals).

## Decisions

### 1. Add `<T, U>` at three declaration sites, reusing existing tokens

`Sig` gains `type_params: Vec<TypeParam>` where
`TypeParam { name: String, span: Span }`. `Sig::type_params` is parsed right
after the function name and before `(`, inside the one `sig()` parser
function already shared by free functions, trait methods, and impl methods —
so `fn map<T, U>`, a trait's `fn map<U>`, and an impl's `fn map<U>` all gain
the syntax for free from one change. `Item::Trait` gains
`type_params: Vec<TypeParam>` parsed after the trait name. `Item::Impl` gains
`type_params: Vec<TypeParam>` parsed right after the `impl` keyword.

Grammar:
```
type_params ::= '<' ident (',' ident)* '>'
fn_head     ::= 'fn' ident type_params? '(' ... ')'
trait_decl  ::= 'trait' ident type_params? '{' ... '}'
impl_decl   ::= 'impl' type_params? (trait_ref 'for')? type '{' ... '}'
trait_ref   ::= ident ('<' type (',' type)* '>')?
```
No new tokens are needed: `Tok::Less`/`Tok::Greater` are already lexed for
`with slot<Type>` and `BinOp` never uses `<`/`>`, so there is no ambiguity
with comparison or generic syntax at these fixed, keyword-anchored positions.

Alternative considered: reuse square brackets (`fn map[T, U]`) to avoid any
token reuse question. Rejected — MAP-Q1 already fixes `<T, U>` as the spelling,
and square brackets are already the array-type sigil.

### 2. Widen `Item::Impl`'s trait reference and target type

`trait_name: Option<String>` becomes `trait_ref: Option<TraitRef>` where
`TraitRef { name: String, args: Vec<Type> }` (`args` empty for a non-generic
trait reference, e.g. today's `impl Database for Postgres`). `type_name:
String` becomes `target: Type`, using the existing type grammar, so `[T]`
parses the same way an array type does everywhere else. Every existing
non-generic `impl Trait for Name` / `impl Name` call site is a mechanical
update to read `trait_ref.name` / `target.name()` instead of the old bare
strings; behavior is unchanged because a non-generic impl's `target` is
always `Type { kind: Named(name), .. }` with no parameters, which round-trips
to the same string.

Alternative considered: keep `type_name: String` and add a parallel
`Option<Type>` used only when the impl is generic. Rejected — two
representations of the same concept invites drift, and every consumer would
still need to handle both; a single `Type` field is one representation, one
consumer contract.

### 3. Type parameter names resolve to a per-declaration scope, not a global table

Type parameter *references* inside a declaration's own type annotations stay
spelled as ordinary `TypeKind::Named(name)` in the AST — the same leaf a
declared type uses. Nothing downstream of parsing can tell "is this name a
type parameter" from syntax alone; that question is answered once, during the
same pass that already turns names into `hir` declaration IDs
(`typecheck::lower_type` / `Nominal`).

When that pass processes a `fn`, trait method, or impl method, it first builds
a local scope: the declaration's own `type_params`, plus (for an impl method)
the enclosing `impl`'s `type_params`. Building this scope is where duplicates
are caught (a name repeated within one `type_params` list, or a method
redeclaring a name its enclosing `impl` already declared) — reported with the
span of the offending declaration. Resolving a `Named(name)` type in the
declaration's own params, return type, trait reference arguments, or target
type then checks this scope first: a hit resolves to a new
`hir::TypeParamId` (a dense per-checked-program `Id`, following the existing
`ids!` macro pattern used for `StructId`/`EnumId`); a miss falls through to
today's existing struct/enum/builtin lookup and, if that also misses, today's
"undeclared type" diagnostic — unchanged wording, now also the mechanism that
rejects a type-parameter-shaped name used outside its declaring scope, since
outside that scope the name is just another unresolved identifier.

Module-level name resolution (`module::resolve_type`) is untouched: it already
leaves any name it cannot match against a local/imported declaration
unchanged (`resolve_parts` falls through to `parts.to_vec()`), so a bare type
parameter name such as `T` passes through this pass as-is, to be classified
later by the scope check above.

Alternative considered: give type parameters their own AST-level
`TypeKind::Param` variant, decided at parse time from the parser's currently
open `<T, U>` list. Rejected — it would force every existing exhaustive match
over `ast::TypeKind` (module resolution, `report_unknown`, dump/render) to
grow a case that means nothing until a later pass exists to interpret it, for
no behavioral gain over resolving the same distinction once, at the same
point names already become IDs today.

### 4. A separate generic signature representation keeps type parameters out of `hir::Type`

Add `hir::TypeParamId` (via the existing `ids!` macro) and a small
`hir::GenericType` enum mirroring `TypeKind`'s shape (`Builtin`, `Named`
referencing a real `StructId`/`EnumId`/`TraitId`, `Array`, `Callable`) plus one
additional case, `Param(TypeParamId)`, for a resolved reference to an in-scope
type parameter. A generic declaration's parameter types, return type, trait
reference arguments, and target type are recorded as `GenericType`, alongside
its ordered `Vec<TypeParamId>`, in a new arena keyed by declaration (e.g.
`GenericFnId`) — populated but not wired into `Ids`/`Nominal`'s existing
callable/method/trait-impl tables, so it is inert with respect to every
existing pass.

`hir::Type`/`hir::TypeKind` gain no new variant. This is the "no type variable
reaches concrete HIR" invariant made structural rather than merely checked at
runtime: there is no `hir::TypeKind` case a type parameter could occupy, so
every existing exhaustive match over it — ownership's Copy classification,
requirement analysis, the interpreter, `wasm_layout`, `wasm_data`, `wasm.rs` —
compiles and behaves exactly as before, untouched by this change. The
checkable seam this task adds is a `GenericType::is_concrete(&self) -> bool`
(and an analogous whole-signature check) that returns `false` exactly when a
`Param` occurs anywhere in the tree; MAP-020's substitution step is expected
to call this (or an equivalent `try_into_concrete`) and treat `false`/`None`
as an internal invariant violation once real call-site instantiation exists.
MAP-010 tests this seam directly by constructing `GenericType` values with and
without `Param` and asserting the check's result, plus a regression test that
`hir::TypeKind`'s variants are all concrete (guards against a future edit
accidentally adding a type-variable case to the wrong enum).

Alternative considered: add `TypeKind::Param(TypeParamId)` directly to
`hir::TypeKind` and rely on a runtime assertion (e.g. in `CheckedProgram`
construction) that no `Param` survives into a checked program. Rejected —
until MAP-020 exists, nothing would ever construct such a value, so the
assertion is untestable except by deliberately misusing an internal
constructor; keeping `hir::Type` structurally incapable of it is a stronger,
equally cheap guarantee and avoids adding dead match arms across five modules
today.

### 5. Generic declarations are parsed and validated, then set aside

`typecheck`'s declaration pass recognizes a `fn`, trait, or `impl` with a
non-empty `type_params` (or, for an impl method, an enclosing impl with one)
and routes it to the new generic-signature construction in Decision 4 instead
of the existing concrete-signature path. It does not lower the declaration's
body to checked HIR, does not register it under `Ids::fns` /
`Ids::trait_methods` / `Ids::methods` / `Ids::trait_impls`, and does not run
the existing whole-body expression checker over it. Consequently requirement
analysis, ownership checking, the interpreter, and Wasm generation never see
a generic declaration, exactly satisfying "generic 宣言は接続しない" for this
task. A `main`/differential run over a program containing only generic
declarations besides `main` itself behaves as if they were absent (still
possible to observe that they parsed, via a direct unit test on the checked
`GenericType`/`GenericFnId` output, without a CLI-level dump command).

This mirrors how `Item::Effect`/`Item::Struct` already coexist as declarations
that do not themselves produce a callable body — no new top-level dispatch
concept is introduced.

### 6. Testing

Focused unit tests in `parse.rs` for: type parameter list parsing and spans
on `fn`/`trait`/`impl`; rejection of a type parameter list on `struct`/`enum`;
`impl<T> Map<T> for [T]` round-tripping through the new `TraitRef`/`target`
fields. Focused tests in `module.rs` confirming an unresolved bare `T` passes
through module resolution unchanged. Focused tests in `typecheck.rs` for:
duplicate type parameter names (own list and impl/method collision) reported
with span; a type parameter name used outside any declaring scope hitting the
existing "undeclared type" diagnostic; a well-formed generic `fn`/`trait`/
`impl` producing a `GenericType`/`GenericFnId` with the expected `Param`
references and no entry in `Ids`'s callable/method/trait-impl tables.
Focused `hir.rs` tests for `GenericType::is_concrete` and the `hir::TypeKind`
concreteness regression test. CLI-level tests in `tests/cli.rs` for the
duplicate and out-of-scope diagnostics' rendered text and span. A full-suite
run (`cargo test`, `cargo fmt --check`, warnings-as-errors Clippy) confirms
every existing AST/HIR/interpreter/Wasm test is unaffected, and a
differential/byte-determinism run over the existing fixtures (unchanged,
since no fixture uses this syntax yet) confirms the pipeline's observable
behavior is untouched.

## Risks / Trade-offs

- [Widening `Item::Impl`'s fields touches every existing reader of
  `trait_name`/`type_name`] → Mechanical, compiler-guided change (the fields
  are renamed and retyped, so every call site fails to compile until fixed);
  covered by the full existing `impl`-related test surface, which must keep
  passing unchanged.
- [Resolving type parameters only at the `typecheck` boundary means a
  generic declaration's body can still contain other, unrelated errors that
  this task does not attempt to diagnose] → Explicitly deferred to MAP-020,
  which does full-body checking with rigid type variables; MAP-010 only
  validates the declaration's own signature shape (params, return, trait
  reference, target), consistent with the roadmap's task boundary.
- [A `GenericType` representation invented now could need reshaping once
  MAP-020 designs substitution] → Keep it minimal (mirrors `TypeKind` plus one
  `Param` case) and privately used only by this task's declaration pass, so a
  later reshape touches one struct and its direct construction site, not
  `hir::Type` or any pass that already matches it exhaustively.
- [Reusing `sig()` for `fn`/trait method/impl method type parameters could
  let a trait method redeclare its enclosing impl's type parameter name
  without an explicit rule] → Covered by a focused test (Decision 3,
  Requirement scenario "An impl method's own type parameter shadows nothing
  it does not declare") and a span-reported duplicate diagnostic if a method
  reuses its enclosing impl's parameter name.

## Migration Plan

1. Land AST changes (`Sig`, `Item::Trait`, `Item::Impl` widening) and parser
   support behind the existing full test suite, with `docs/grammar.md`
   updated in the same commit.
2. Add `hir::TypeParamId`/`GenericType` and the `typecheck` declaration-scope
   resolution (duplicate/out-of-scope diagnostics, `GenericType` construction,
   exclusion from `Ids`), with focused tests.
3. Add CLI diagnostic tests, the `hir::TypeKind` concreteness regression test,
   and run the full verification suite (fmt, Clippy, tests, differential
   determinism) before committing the final stable snapshot.
4. Rollback is a plain revert: no persisted data, public ABI, or host
   integration is introduced, and no existing fixture uses this syntax.
