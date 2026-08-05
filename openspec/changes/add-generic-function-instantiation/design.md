## Context

See `proposal.md` for motivation and `specs/generic-function-instantiation/spec.md`
for the contract. This picks up exactly where
`openspec/changes/archive/2026-08-05-add-generic-type-parameters/` (MAP-010)
left off.

MAP-010 gives every generic `fn`/trait method/impl method a `hir::GenericDecl`
(`src/hir.rs`): an ordered `Vec<TypeParamId>` plus `params`/`ret` (and, for an
`impl`, `trait_args`/`target`) as `hir::GenericType` — a tree shaped exactly
like `hir::Type` but with one extra leaf, `GenericTypeKind::Param(TypeParamId)`,
for a reference to one of the declaration's own type parameters. `GenericDecl`s
live in `Program.generics`, keyed by `GenericFnId`, and are populated by
`typecheck::record_generic` during `collect()`'s declaration pass. They are
deliberately not registered in `Ids::fns`/`trait_methods`/`methods`/
`trait_impls`, and `check_and_lower`'s second pass (which checks bodies)
explicitly skips any `Item::Fn`/`Item::Impl` method for which
`is_generic(..)` is true. So today: the signature is validated and recorded,
the body is never looked at, and no call can resolve to one.

Ordinary (non-generic) body-checking already goes through one pipeline:
`check_and_lower`'s second pass calls `check_body(body, Some(sig), receiver,
decls, ctx, ret, target, lowered, out)`, which allocates local bindings from
`sig.params` (converted to a source-level `KnownType` by `known()`), then
`sequence`/`walk`s the body with `Cx { decls, ctx, ret }` in scope. `KnownType`
(`typecheck.rs`) is the type representation live during a single body-check:
its `KnownKind::Named(String)` leaf holds a source-spelled name (a builtin, or
whatever `known()` copied verbatim from an AST `TypeKind::Named`), and
equality/`fits` compare it structurally — nothing about `KnownType` requires
its `Named` string to resolve to a real declared type until `lower_known`
turns it into `hir::Type` for storage in the `hir::Body` being built.
`lower_known` resolves a `Named` leaf against `builtin()` then
`Nominal.types`, defaulting silently to `hir::TypeKind::Poison` on a miss;
separately, exactly one place in body-checking — the `let` annotation branch
of `walk` — proactively calls `report_unknown(annotation, &decls.nominal,
out)` to diagnose an undeclared name eagerly. `Target::Discard` already exists
for "check this body for diagnostics, but there is nowhere to keep the
resulting HIR" (used today for a method whose `impl` target isn't a struct).

Call resolution (`typecheck::resolve`/`call`) picks a `CallTarget` (`Direct`,
`Method`, `Associated`, `Slot`, `Ctor`, `Indirect`) and an `FnSig` once, then
checks each argument against the resolved parameter types
(`argument`/`conform`, which also inserts an automatic shared borrow at a
`&T` parameter). `hir::Call::Direct { callable, args }` is exactly the same
node an ordinary top-level call produces; nothing about ownership checking
(`src/ownership.rs`), requirement analysis (`src/requirement.rs`), the
interpreter (`src/eval.rs`), or Wasm generation (`src/wasm.rs`) is specific to
*how* a `CallableId` came to exist — they all iterate `Program.callables`
generically and process whatever they find.

## Goals / Non-Goals

**Goals:**
- Check a generic free function's body exactly once, with its own type
  parameters acting as rigid, self-distinct opaque types, independent of any
  call site, reusing the existing single-pass expression checker.
- At a call to a generic free function, infer every type argument purely from
  the checked types of the call's arguments (MAP-Q2); diagnose an unresolved
  or conflicting type argument before execution.
- Turn a solved call into a concrete, monomorphized `hir::Callable`, cached by
  `(GenericFnId, type arguments)` so repeat calls share one instance and a
  same-arguments recursive call does not loop the compiler.
- Diagnose polymorphic recursion (one generic declaration recursively
  requiring a different type-argument instantiation of itself) before
  attempting to build it.
- Make a concrete instantiation ordinary enough that ownership checking, the
  interpreter, and Wasm generation need zero new code to run it.

**Non-Goals (explicitly deferred, per ROADMAP.md):**
- Generic trait methods and generic impl methods, their contract checking,
  and method resolution — MAP-025. This task only instantiates generic free
  functions declared with `fn name<T>(...)`.
- Widening the specialization key with callback identity or trait impl
  choice, and whole-program-scale reachability-based generation — MAP-040.
  See "Decision 6" for exactly what key this task uses instead and why that
  is sound for this task's scope.
- Per-callback ambient/effect requirement inference — MAP-050. The example
  bodies this task ships (`identity`, `apply`) introduce no ambient
  requirement, so this gap is not exercised by anything this task tests.
- Diagnosing a polymorphic-recursion cycle spanning more than one distinct
  generic declaration (mutual recursion between two generics) — left to
  MAP-040's whole-program specialization graph.
- Explicit call-site type-argument syntax (MAP-Q2) — not introduced; see
  Decision 4 for why the parser already rejects the shape without new code.

## Decisions

### 1. Rigid body-checking reuses `check_body`, with type parameters standing in as fresh, self-only-equal `KnownType` names

For each generic free function, `check_and_lower`'s second pass gets a new
sibling arm next to the existing `if !is_generic(&[], sig)` branch: when
`is_generic` is true, look up the `GenericDecl` already built for this
declaration (via a new `Ids.generic_fns: BTreeMap<String, GenericFnId>` index,
populated next to the existing `record_generic` call — the same shape as the
existing `Ids.fns` index for non-generic functions), and check its body with
`Target::Discard` so the resulting HIR is thrown away (nothing downstream
ever reads a `Target::Discard` output — see `lower_impl`'s existing use of
the same target for a method whose `impl` doesn't resolve).

Each of the declaration's `TypeParamId`s gets a synthetic `KnownKind::Named`
spelling that cannot collide with any real name: `#T<index>` (built from the
dense `TypeParamId` index; the lexer never produces `#` inside an identifier,
so no user or module-qualified name can ever equal one). `GenericDecl.params`/
`.ret` (`GenericType`) are converted to `KnownType` by walking the same shape
`known()` already walks, mapping `Param(id)` to that synthetic name and every
other case 1:1 (`Builtin(b)` → `Named(b.spelling())`, `Struct(id)`/`Enum(id)`
→ `Named(<that declaration's canonical name>)`, `Array`/`Callable` recurse).
This reuses the *existing* `KnownType`/`walk`/`fits`/`conform` machinery
completely unchanged: `fits` already compares `KnownKind::Named` structurally
by string equality, so two occurrences of the same type parameter unify only
with each other, and a type parameter never accidentally fits a builtin or a
declared type it isn't.

Because the synthetic name is registered *nowhere* — not in `Nominal.types`,
not in `decls.structs`, not in `decls.impls` — a type parameter used as a
struct literal, a field access, or a method-call receiver fails to resolve
exactly like any other undeclared name, with zero new rejection code: opacity
falls out of "this name resolves to nothing," not out of an explicit deny
rule. The one place that needs an explicit allowance is the `let` annotation
branch of `walk`, which eagerly calls `report_unknown(annotation,
&decls.nominal, out)` — this would otherwise misreport a bare `T` written in
a `let` annotation inside a generic body as an undeclared type. `Cx` gains a
fourth field, `type_params: &'a TypeParamScope` (a `BTreeMap<String,
TypeParamId>`; every existing call site passes a shared empty map, so
non-generic checking is byte-for-byte unchanged), and `report_unknown` gains
the same scope parameter, short-circuiting to "declared" for a name the scope
contains. This is the only change to code that runs for every body-check,
generic or not, and it is a no-op change for the non-generic case.

`check_body` itself changes shape slightly: instead of deriving parameter
names/types from `Option<&Sig>` internally, it takes `params: &[(String,
KnownType)]` and `span: Span` directly. The three existing call sites (free
fn, impl method, test) each build that list the same way they always
computed it (`sig.params.iter().map(|p| (p.name.clone(), known(&p.ty)))`, or
empty for a test); the new generic-body call site builds it by zipping the
AST `Sig.params[i].name` (still available — this pass iterates
`Item::Fn { sig, body, .. }` directly) with the synthetic `KnownType`s above.
This is a mechanical decoupling, not a behavior change for existing callers.

**Alternative considered:** give `KnownKind` a real `Param(TypeParamId)`
variant instead of a synthetic string. Rejected — it would require every
existing match over `KnownKind` (`fits`, `lower_known`, `Display`, the
struct/array/callable helpers) to grow a case purely to be immediately
discarded by `Target::Discard`, for a body-check pass whose output nothing
ever reads. The synthetic-name encoding gets the same rigidity and opacity
guarantees by construction, with zero new match arms.

**Alternative considered:** allocate a real zero-field phantom `StructId` per
type parameter and register it in `Nominal.types`/`decls.structs`/
`Program.structs`, so a type parameter looks like an ordinary struct type to
every existing lookup. Rejected — it would (a) let a generic body illegally
write `T{}` (a zero-field struct literal type-checks against a zero-field
phantom struct, silently defeating opacity) and (b) leak permanent,
synthetically-named entries into `Program.structs`, which ownership and Wasm
generation iterate unconditionally, polluting dumps/differential fixtures for
programs that use generics at all. The chosen design never registers the
synthetic name anywhere, so opacity is structural and no arena gains
unrelated entries.

### 2. Call-site inference unifies each argument's already-lowered `hir::Type` against the callee's `GenericType`, producing a `BTreeMap<TypeParamId, hir::Type>`

`resolve()` gains a branch: if `ExprKind::Ident(name)` doesn't match
`decls.fns` (an ordinary function) but does match the new
`decls.generic_fns` index, resolve it as a generic call instead of failing
with "unknown name."

Per MAP-Q2, a generic call's arguments carry *all* the information type
arguments come from — there is no expected-type-first checking here, unlike
an ordinary call. So this branch `synth`s (walks with no expected type) each
argument independently first, exactly the shape `argument()` already falls
back to when no expected type applies. This is also the answer to why a bare
`nil` or an un-annotated empty array literal passed to a generic parameter is
"unresolved": neither can synthesize a type without an expected type to
consult, and a generic call site is not one — that is what MAP-Q2 means by
"型引数を推論できない…は今回扱わない," not a separate rule this task adds.

Each argument's checked `KnownType` is lowered to `hir::Type` the same way it
would be if it were the RHS of a `let` with no annotation (`lower_known`,
already safe here — a call-site argument in non-generic code is, by
construction, already fully concrete, never a rigid placeholder). Unification
then walks the callee's `GenericDecl.params[i]` (a `GenericType`) against that
`hir::Type` structurally:

- `Param(id)` against any `hir::Type`: if `id` is already bound, require the
  new occurrence to equal the existing binding exactly (`hir::Type: PartialEq`
  already gives this); otherwise bind it.
- `Builtin`/`Struct`/`Enum` against the same `TypeKind` variant with the same
  ID/builtin: direct comparison — both sides are already ID-based here (no
  string bridging needed, unlike `KnownType`'s name-based comparison).
- `Array`/`Callable`: recurse structurally; shape mismatch is a unification
  failure.
- `reference`/`optional` on both sides must match exactly at each position
  (both `GenericType` and `hir::Type` carry these the same way `Type` does).

After walking every parameter, every `TypeParamId` in
`GenericDecl.type_params` must have a binding: a missing one is "unresolved,"
diagnosed with the type parameter's name (`Program.type_params[id].name`) and
the call's span. A binding conflict encountered mid-walk is "conflicting,"
diagnosed with the type parameter's name and both inferred types (reusing
`Program.show_type` for the message, the same call other diagnostics already
make). Both diagnostics fire before any instantiation is attempted and before
any argument is otherwise used — satisfying "実行前に診断する" for this task's
two failure shapes.

Once solved, the substitution also gives the concrete expected type for each
parameter (substitute `GenericDecl.params[i]` through the solved map — see
Decision 3). Each already-`synth`ed argument is passed through the existing
`conform(checked, &expected, site, span, cx, out)` (unchanged), which
re-validates the (already-guaranteed-to-fit, since it is literally where the
binding came from) shape and inserts an automatic shared borrow exactly where
a non-generic call already would for a `&T`-shaped parameter. `resolve()`
returns these already-lowered argument IDs directly in `Resolved` so `call()`
does not walk the arguments a second time (a new `Resolved.args:
Option<Vec<hir::ExprId>>`, `Some` only for this branch — every other
`CallTarget` keeps working through `call()`'s existing per-argument loop
unchanged).

**Alternative considered:** thread an expected type into each argument the
way an ordinary call does, by first "guessing" the parameter shape from
argument position. Rejected — MAP-Q2 already decided type arguments come
*from* concrete arguments, not the reverse; threading a not-yet-known
expected type into argument checking would need a second, different kind of
inference (bidirectional/occurs-check style) this milestone does not ask for,
and would change which programs are accepted beyond what the two flagship
examples need.

### 3. Substitution and monomorphization produce an ordinary `hir::Callable`, cached by declaration and type arguments

Given a solved `subst: BTreeMap<TypeParamId, hir::Type>` covering every one of
`GenericDecl.type_params`, a new `substitute(generic_ty: &hir::GenericType,
subst) -> hir::Type` in `hir.rs` walks `GenericType` and replaces `Param(id)`
with `subst[id].clone()`, mapping every other case 1:1 (this is the mechanical
counterpart to `GenericType::is_concrete`, and its result satisfies that
predicate by construction whenever `subst` is total over the declaration's
type parameters, which call-site inference already guarantees before this
runs). This gives the instantiation's concrete parameter/return types.

The specialization key for this task is `(GenericFnId, Vec<hir::Type>)`, where
the vector is `GenericDecl.type_params.iter().map(|id| subst[id].clone())` —
declaration identity plus the ordered, fully concrete type arguments, nothing
else (see Decision 6 for why this key is intentionally narrower than MAP-Q5's
eventual whole-program key). `check_and_lower` keeps a `Vec<(GenericFnId,
Vec<hir::Type>, CallableId)>` cache alongside its other per-run state; a
lookup that finds an existing entry reuses its `CallableId` immediately,
producing `CallTarget::Direct(that id)` with no further work. `hir::Type`
already derives `PartialEq`/`Eq`, so comparing keys needs no new trait impl;
a linear scan is used rather than a `BTreeMap` (which would need `Ord` added
to `hir::Type`/`TypeKind`/`RefKind`/`Builtin` purely for this cache) — the
number of distinct `(generic, type-args)` pairs in any program this milestone
targets is small, and this avoids adding an ordering to types that no other
part of the compiler needs.

On a cache miss, before checking the body: allocate the concrete `hir::Callable`
shell (name, concrete params/ret, `CallableOwner::Free`, empty body) in
`lowered.callables` — exactly the same "shell first, body second" order
`collect()`'s declaration pass already uses for ordinary recursive functions
— insert `(GenericFnId, type_args) → that CallableId` into the cache *before*
checking the body, and push `(GenericFnId, type_args)` onto a small
"currently instantiating" stack. Then check/lower the body with
`Target::Callable(shell_id)`, using the *same* substituted `KnownType`s built
in Decision 1 as the parameter/receiver types, but this time letting
`report_unknown`/`lower_known` resolve every leaf for real (there is no
`Param` left anywhere — every leaf is a genuine builtin/struct/enum/array/
callable name), so the produced `hir::Body` is fully concrete and gets kept.
A call inside this body back to the same generic declaration goes through the
exact same `resolve()` path from Decision 2; if it resolves to the same
`(GenericFnId, type_args)` key already on the stack, the cache lookup finds
the reserved (still being filled) `CallableId` and returns
`CallTarget::Direct` immediately, without re-entering this construction (this
is what makes same-arguments recursion terminate — see Decision 5). Once the
body finishes, pop the stack entry.

**Alternative considered:** re-lower the callee's body once and keep the
substitution as a rewrite applied lazily at each use (an interpreter-side
"generic frame"), instead of eagerly producing one `hir::Callable` per
distinct type-argument tuple. Rejected — MAP-Q5 already decided ownership
must see fully concrete HIR with no type variable, and the milestone's
non-goal list excludes closures/dynamic dispatch machinery; eager
monomorphization is also what makes "zero new code in ownership/eval/wasm"
possible, since those passes already assume every `Callable` they see is
already fully concrete.

### 4. No explicit type-argument call syntax is introduced

MAP-Q2 already decided type arguments are inferred, not spelled at a call
site. `ast::BinOp` has no `<`/`>` variant (confirmed in MAP-010's design.md
decision 1 and unchanged by this task), and no grammar production for a type
argument list exists after a call's callee expression. A source attempting
`identity<int>(5)`-shaped syntax therefore already fails to parse today, with
whatever diagnostic the existing expression grammar produces for an
unexpected token after a bare identifier — this task adds no new parser rule
and no new typecheck-level rejection for this case; the "明示的な型引数指定を
実行前に診断する" completion criterion is met by the parser's existing,
unchanged behavior.

### 5. Same-arguments recursion shares a reserved slot; only self-recursion is diagnosed for polymorphic recursion, matching this task's boundary

Decision 3's "shell first, cache the key immediately, then fill the body"
order is precisely MAP-Q5's "同じキーの再帰は具体化枠を先に確保して共有する"
applied to the one shape this task can produce: a generic free function
calling itself. Because the cache is consulted *before* recursing into the
body, the compiler's own instantiation process cannot loop as long as the
recursive call resolves to the same key.

Polymorphic recursion — the same generic declaration's body requiring a
*different* type-argument instantiation of itself while the first is still
being built — is detected using the same "currently instantiating" stack:
before pushing a new `(GenericFnId, type_args)` onto it, if the stack already
contains an entry with the same `GenericFnId` but different `type_args`, that
is polymorphic recursion. The diagnostic reports the declaration's name, both
type-argument tuples (shown via `Program.show_type`), and the call site's
span; nothing further is attempted for that call. Because the stack only
tracks entries for the declaration whose body is currently being built and
its transitive callees within *this task's* reach (free-function calls only —
no trait/impl dispatch exists yet, so there is no indirection through which a
cycle could hide), this construction only ever detects a cycle through a
single generic declaration. A cycle that alternates between two or more
distinct generic declarations calling each other is invisible to a single
declaration-keyed check and is explicitly left to MAP-040, which owns the
whole-program specialization graph where such a cycle is actually visible as
a graph property rather than a per-declaration stack.

### 6. The specialization key omits callback identity and trait impl choice; this is sound for this task's scope

MAP-Q5's decision text names three components of the eventual specialization
key: generic declaration ID, type arguments, and callback binding. This task
only ever produces a key of the first two. This is deliberately narrower than
MAP-Q5's end state, and is safe for this task's boundary for a specific
reason: nothing this task wires up yet distinguishes behavior by *which*
concrete function value is bound to a callable-typed parameter.

- The interpreter already resolves a callable-typed parameter's calls
  (`f(x)` inside `apply`'s body) dynamically at the call site via
  `hir::Call::Indirect`, reading whatever function value the caller actually
  passed at run time — this is the existing mechanism for named function
  values in general (predates generics) and does not change per
  instantiation.
- Ambient/effect requirement analysis walks a callable's body once and
  attributes requirements to it; today it has no way to see "through" a
  callable-typed parameter to whatever specific function was passed at a
  particular call site, generic or not (that is exactly MAP-050's job). So
  under this task, two calls to the same generic declaration with the same
  type arguments but different callback arguments already produce identical
  requirement-analysis results (none, for `identity`/`apply`, since neither
  declares or uses an ambient effect) — sharing one instantiation between
  them changes nothing observable.

Once MAP-050 makes requirement analysis attribute effects through a
callback-typed parameter to the specific function bound to it, sharing one
instantiation across different callback arguments would become observably
wrong (two different callbacks with different ambient needs could not be
told apart from a single shared instantiation). MAP-040 is exactly where the
key widens to prevent that, before MAP-050 needs it. Until then, this task's
narrower key produces correct results for everything it can express, at the
cost of being coarser than the eventual whole-program key — an intentional,
temporary imprecision, not a correctness gap in what this task itself claims
to support.

### 7. Determinism

Instantiations are created in the order their originating call is first
reached in `check_and_lower`'s existing, single-threaded, deterministic
second pass (file order over items, then `walk`'s existing deterministic
expression order within a body) — the same order that already determines
every other decision this compiler makes deterministic (ID allocation order,
diagnostic order). No new source of nondeterminism (hashing, concurrency, or
iteration over an unordered collection) is introduced; the instantiation
cache is a `Vec`, scanned linearly, and appended to in discovery order.

## Risks / Trade-offs

- [Decoupling `check_body`'s parameter list from `Option<&Sig>` touches all
  three existing call sites] → Mechanical, compiler-guided (each call site's
  parameter list is either the same `sig.params` derivation already inline,
  or empty for a test); covered by the full existing body-checking test
  suite, which must keep passing byte-for-byte unchanged.
- [Threading `type_params: &TypeParamScope` through `Cx` and `report_unknown`
  adds a parameter to a hot, shared path] → Every existing call site passes a
  shared empty map, so this is a zero-behavior-change addition for every
  non-generic check; a focused test confirms a non-generic program's
  diagnostics and HIR are byte-identical before and after.
- [The synthetic `#T<index>` name scheme silently relies on the lexer never
  producing `#` inside an identifier] → Covered by a regression test
  asserting the lexer rejects `#` as an identifier character (guards against
  a future lexer change accidentally opening a collision).
- [A narrower specialization key (Decision 6) could look like premature
  under-specialization to a later reader] → Decision 6 states precisely why
  it is sound for this task's own reach and precisely which later task
  (MAP-040) is responsible for widening it; the `generic-function-instantiation`
  spec's Non-Goals section repeats this boundary so it is visible from the
  contract, not just this document.
- [Self-recursion sharing (Decision 5) could mask a genuine polymorphic
  recursion bug if the "currently instantiating" stack were popped too
  early, or scoped incorrectly across sibling calls] → Covered by a focused
  test pairing a legitimate same-type recursive generic function (must
  compile once, to one instantiation) against a minimal polymorphic-recursion
  fixture (must be rejected), both exercising the same stack.
- [Building the concrete instantiation re-walks the callee's body once per
  distinct type-argument tuple, in addition to the one rigid whole-body
  check] → Accepted; this is the cost of eager monomorphization (Decision 3),
  bounded by the number of distinct call-site type-argument tuples in the
  program, consistent with the "検査済みHIRとCheckedProgramに型変数は残さない"
  decision already made in MAP-Q5.

## Migration Plan

1. Add `Cx.type_params`/`report_unknown`'s scope parameter and decouple
   `check_body`'s parameter list from `Option<&Sig>`, behind the existing
   full test suite, confirming zero behavior change for every non-generic
   program.
2. Add the rigid whole-body check for generic free functions (`Ids.generic_fns`,
   the synthetic-name conversion, the new `check_and_lower` arm targeting
   `Target::Discard`), with focused tests for accepted/rejected generic
   bodies independent of any call.
3. Add `hir::substitute`, call-site unification in `resolve()`, and the
   unresolved/conflicting diagnostics, with focused tests for `identity`/
   `apply` success and both failure shapes.
4. Add the instantiation cache, shell-first construction, the "currently
   instantiating" stack, and the polymorphic-recursion diagnostic, with
   focused tests for sharing, distinct-type-argument instantiation, safe
   same-type recursion, and rejected polymorphic recursion.
5. Add CLI-level success tests (`identity`/`apply` at multiple concrete
   types, run to completion, including a Wasm build byte-determinism check)
   and CLI-level failure tests (unresolved, conflicting) before committing
   the final stable snapshot.
6. Rollback is a plain revert: nothing here changes a public ABI, persisted
   format, or non-generic program's behavior; no existing fixture calls a
   generic function today.
