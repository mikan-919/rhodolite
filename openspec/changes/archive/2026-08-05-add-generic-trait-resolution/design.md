## Context

See `proposal.md` for motivation and `specs/generic-trait-resolution/spec.md`
for the contract. This picks up where
`openspec/changes/archive/2026-08-05-add-generic-type-parameters/` (MAP-010)
and `openspec/changes/archive/2026-08-05-add-generic-function-instantiation/`
(MAP-020) left off.

MAP-010 gives every generic `fn`, trait method, and impl method a
`hir::GenericDecl` (`src/hir.rs`): an ordered `Vec<TypeParamId>` plus
`params`/`ret` as `hir::GenericType` (a tree shaped like `hir::Type` with one
extra leaf, `GenericTypeKind::Param(TypeParamId)`). A trait method's
`GenericDecl.owner` is `GenericOwner::Trait(TraitId)`; an impl method's is
`GenericOwner::Impl { trait_: Option<TraitId>, trait_args: Vec<GenericType>,
target: GenericType }`. `GenericDecl.type_params` is the enclosing
declaration's own parameters followed by the method's own — for a trait
method this is `trait`-level params then the method's own; for an impl method
it is `impl`-level params then the method's own. None of this is registered
in `Ids::trait_methods`/`methods`/`trait_impls`, so today a generic trait
method's contract is never checked against any impl, and a call can never
resolve to one — MAP-010's `collect()` even validates `trait_ref.name`
against `nominal.traits` for a generic impl but silently drops the result
into `trait_: None` on a miss, with a comment marking exactly this as
"generic の契約検査(MAP-025)の範囲" (`src/typecheck.rs`, in the `Item::Impl`
declaration-pass branch). A generic impl method's *body* is not checked at
all today, not even rigidly — `check_and_lower`'s body-checking pass
`continue`s over any impl method for which `is_generic(type_params, sig)` is
true (`src/typecheck.rs`, the `Item::Impl` arm of the main body-checking
loop).

MAP-020 built the machinery this task reuses for a generic *free* function:
`check_rigid` type-checks a generic body once with each type parameter
standing in for a synthetic, collision-free `KnownType` name (`#T<index>`,
via `known_generic`/`rigid_name`), so the body is opaque to anything not
already valid for an arbitrary unknown type. `resolve()`'s `generic_call`
`synth`s each call argument, then `unify()`s the callee's declared
`GenericType` parameters against each argument's already-lowered `hir::Type`,
producing a `BTreeMap<TypeParamId, hir::Type>` (diagnosing "unresolved" for a
parameter no argument bound, "conflicting" for one bound to two different
types). `instantiate()` then builds a concrete `hir::Callable` shell,
inserts it into a `Vec<(GenericFnId, Vec<hir::Type>, CallableId)>` cache
*before* checking the substituted body (so same-key recursion shares the
reserved shell and a different-key recursion is diagnosed as polymorphic
recursion via a `Vec<(GenericFnId, Vec<hir::Type>)>` "currently instantiating"
stack), then checks/lowers the body against the substituted, now-fully-
concrete `KnownType`s. The produced `CallableId` becomes an ordinary
`CallTarget::Direct`, so ownership, requirement analysis, the interpreter,
and Wasm generation need zero new code to run it — they already process
`Program.callables` generically.

Non-generic method-call resolution (`resolve()`'s `ExprKind::Field` arm)
already exists and is unaffected by MAP-010/020: it `synth`s the receiver,
and if its type has a name (`KnownType::name()` — `None` for an array or
callable type), looks up `from_type(type_name, name, ...)` against
`decls.impls` (a flat `(method name, FnSig)` table mixing inherent and trait
methods for that type, built once at declaration time) and, if found,
`concrete_target` resolves the unique `CallableId` from `Ids.methods`.
`check_impl` (`src/typecheck.rs`) is the existing declaration-time contract
check for a *non-generic* `impl Trait for Type`: for each provided method it
requires a same-named trait method to exist, requires an exact receiver/
params/ret match (structural equality on `FnSig`, which is built from
`Sig`/`KnownType` with parameter names dropped), and reports every method the
trait declares but the impl omits. It reports these against
`Decls.traits`/`Decls.structs`, which today hold only non-generic
declarations.

`hir::CallableOwner::TraitImpl(TraitImplId)` and `hir::TraitImplDecl { trait_:
TraitId, type_: StructId, methods: BTreeMap<TraitMethodId, CallableId>, .. }`
are the only representation an impl's target has today, and both fix the
target to a `StructId` — this is not new to generics: a non-generic
`impl Trait for [int]` is *already* rejected today by `check_impl`'s
`!decls.structs.contains_key(type_name)` check, since only a struct can be an
impl target at all. `TraitImplDecl.methods` is read only by ambient/slot
dispatch (`Call::Slot`, resolved at the `with`-provided binding) in
`ambient_abi.rs`/`eval.rs`/`wasm.rs`; a statically-dispatched method call
(`Call::Method { callable, recv }`, produced by `CallTarget::Method`) never
reads it — the compiler already picked the concrete `callable` at the call
site. `ownership.rs` does not match on `CallableOwner` at all; it processes
every `hir::Callable`'s body uniformly regardless of how it came to exist.

## Goals / Non-Goals

**Goals:**
- Contract-check a generic `impl`'s methods against its trait's declared
  signatures, with the trait's own type parameters substituted through the
  impl's trait-reference arguments and the trait method's own type
  parameters corresponded to the impl method's own, positionally.
- Validate a generic impl's trait reference (name resolves to a declared
  trait, argument count matches that trait's own arity) — the check MAP-010
  explicitly deferred to this task.
- Type-check a generic impl method's body once, rigidly, independent of any
  call, reusing MAP-020's rigid-checking shape.
- At a method-call site whose receiver's concrete type isn't resolved by
  today's existing (non-generic) method lookup, search declared generic
  impls, infer every remaining type argument from the call's arguments, and
  monomorphize a unique match into an ordinary, cached, concrete
  `hir::Callable` — reusing MAP-020's unification and instantiation-cache
  machinery.
- Diagnose, with a source-positioned span: a trait name or arity mismatch on
  an impl's trait reference; a missing, duplicated, or contract-mismatched
  provided method; an unresolved or conflicting inferred type argument at a
  call site; and a call site whose receiver type is provided by more than one
  generic impl (ambiguous resolution).
- Leave every non-generic `trait`/`impl` declaration and method-call
  resolution byte-for-byte unaffected.

**Non-Goals:**
- An impl target that is not a declared struct (an array, a callable type, a
  builtin, or a blanket `impl<T> Trait<T> for T`) — see Decision 1 for why
  this task's contract-checking and resolution require a struct target, the
  same requirement a non-generic impl already has today, and why this is
  sound for this task's own reach. Widening `hir::CallableOwner`/
  `hir::TraitImplDecl` to a non-struct target is left to whichever task
  first needs it (MAP-075/MAP-080, which build the array `impl` `map` itself
  needs).
- Resolving a generic trait method through ambient/slot dispatch (`with` /
  `db.method()`); this task only resolves the direct method-call syntax
  `recv.method(args)` MAP-Q3 already fixed as `map`'s call shape.
- Widening the specialization key with callback identity, or any
  whole-program-scale reachability pass (MAP-040).
- New ownership rules, or ownership-focused tests, for a generic impl's
  instantiation. This task's instantiations are ordinary `hir::Callable`s
  that ownership already processes with zero new code — the same property
  MAP-020 established for generic free functions — but the dedicated
  ownership-behavior test matrix for this newly-reachable shape (Copy/
  non-Copy receivers and arguments, consuming-`self` moves) is MAP-030's
  scope, per the roadmap's task boundary.
- Constrained type parameters, overload resolution by argument shape beyond
  the single "which impl's target matches this receiver" question, and
  generic `struct`/`enum` (ROADMAP Non-goals; unchanged by this task).
- The real `Map<T>` trait and its array `impl` (MAP-080), and the array
  construction primitive its body will need (MAP-075, `needs-design`).

## Decisions

### 1. A generic impl usable by this task's contract-checking and resolution must target a declared struct, with any of its own type parameters confined to the trait reference and method signatures

`GenericOwner::Impl.target` is a `GenericType` and can syntactically be
anything the impl's own type parameters and the type grammar allow —
including `[T]` (MAP-Q1's flagship `impl<T> Map<T> for [T]`) or, in
principle, a bare `T` (a blanket impl). But `hir::CallableOwner::Inherent`/
`TraitImpl` can only name a `StructId`, and — independent of anything this
task adds — a non-generic `impl Trait for [int]` is *already* rejected today
by `check_impl`'s struct-only check. This task keeps that requirement:
contract-checking and resolution require the impl's target, after
substituting the impl's own type parameters (which, since generic
`struct`/`enum` remain a roadmap non-goal, can never appear *inside* a
struct's name — only stand for the *entire* target or be entirely absent
from it), to name a declared struct. A target whose outer shape can never be
a struct — `Array`, `Callable`, `Builtin`, or the impl's own bare type
parameter (`Param`, a blanket impl) — is rejected at declaration time with
the same "`{type}` は struct ではありません" diagnostic a non-generic impl
already receives for the same reason.

A direct, useful consequence: every generic impl this task can contract-check
and resolve has a **fully concrete** target (a `StructId`, with no `Param`
anywhere in it) — the only place an impl's own type parameters can actually
appear is in its trait-reference arguments and its methods' parameter/return
types, never in `target` itself. Resolution's receiver-side step (Decision 4)
is therefore never asked to unify a `Param` against a concrete receiver type;
every type argument this task infers comes from the call's *arguments*, the
same way MAP-020 already infers a generic free function's type arguments —
Decision 4 reuses that unification unchanged, seeded empty rather than
seeded from the receiver.

**Alternative considered:** widen `hir::CallableOwner`/`TraitImplDecl.type_`
to an arbitrary `hir::Type` now, so `impl<T> Map<T> for [T]` can be
contract-checked *and* resolved end-to-end in this task. Rejected — this
would require deciding how ownership, the interpreter, and Wasm generation
name and dispatch a method whose receiver is an array (today they only ever
see a method receiver as a struct-typed `self`), which is exactly the kind of
new data-representation and runtime-contract question MAP-075 (currently
`needs-design`, precisely over how a body constructs an array result) and
MAP-080 (the real `Map<T>` impl) exist to answer deliberately, not as a side
effect of this task's contract-checking work. Scoping this task to struct
targets lets `Map<T>`'s own contract shape — trait `Map<T>` with method
`map<U>` — still be exercised end-to-end today using a struct-targeted
stand-in impl in tests, without pre-empting that later decision.

### 2. Contract-checking substitutes the trait's own parameters through the impl's trait-reference arguments, then corresponds each trait method's own parameters to the impl method's own by declaration position

A trait's own declared type parameters need a place to live outside the
per-method `GenericDecl.type_params` list (which mixes them with each
method's own). `hir::TraitDecl` gains `type_params: Vec<TypeParamId>` (empty
for a non-generic trait), populated from the same `declare_type_params` call
`collect()`'s `Item::Trait` branch already makes before iterating the
trait's methods. This is a direct, minimal addition — the value already
exists locally in that loop today; it is simply not retained anywhere a
later pass can read it back.

For one generic impl block with trait reference `Trait<A1, ..., Am>`
targeting struct `S`, and one trait method declared as
`GenericDecl { type_params: [t_1, ..., t_n, u_1, ..., u_k], .. }` (the
trait's own `n` parameters — from the freshly-added `TraitDecl.type_params`
— followed by the method's own `k`), contract-checking proceeds in two
substitution passes, both walking the same `GenericType` shape `substitute`
already walks:

1. **Trait-level substitution.** Build
   `subst_trait: BTreeMap<TypeParamId, GenericType> = {t_1: A1, ..., t_n:
   Am}` (the impl's own `lower_generic_type`-produced trait-reference
   arguments, already `GenericType` values in the impl's own type-parameter
   scope). A new `hir::substitute_generic(ty: &GenericType, subst:
   &BTreeMap<TypeParamId, GenericType>) -> GenericType` — `substitute`'s
   direct counterpart, but producing a `GenericType` and leaving an
   unbound `Param` as itself rather than requiring a total binding — replaces
   every `t_i` occurrence in the trait method's receiver/params/ret,
   leaving each `u_j` (the method's own parameters, not in `subst_trait`)
   untouched. Because a well-formed impl's trait-reference arguments are
   built directly from the impl's own type parameters (`impl<T> Trait<T> for
   S` lowers `A1` to `Param(impl's own T)`), this step alone already makes
   every reference to an impl-level trait parameter compare equal, by plain
   `GenericType: PartialEq`, to the impl-provided method's own use of that
   same impl-level parameter — no renaming is needed for this half.
2. **Method-level correspondence.** Zip the trait method's own tail
   `[u_1, ..., u_k]` with the impl-provided method's own declared type
   parameters `[v_1, ..., v_k']`; a count mismatch (`k != k'`) is itself a
   signature-mismatch diagnostic (the impl method takes a different number
   of its own type parameters than the trait declares). Otherwise build
   `corresponds: BTreeMap<TypeParamId, TypeParamId> = {u_1: v_1, ..., u_k:
   v_k}` and apply it (a second, simpler substitution — `Param(u_j)` ↦
   `Param(v_j)`, everything else unchanged) to the already trait-level-
   substituted signature from step 1.

The result is the trait's contract for this method, fully rewritten into the
impl's own type-parameter identifiers. Comparing it against the impl-provided
method's actual `GenericDecl` (`receiver`, `params`, `ret`) is now plain
structural equality — exactly the same three comparisons `check_impl` already
makes for the non-generic case, generalized from `FnSig`/`KnownType` equality
to `GenericType` equality, with the same three diagnostic messages
("レシーバは...ですが...", "引数は...ですが...", "戻り値は...ですが..."). A
non-generic trait's non-generic method is representable in this same
comparison with `n = k = 0` — substitution and correspondence are no-ops —
so this task can use one contract-checking function for both a fully
concrete `impl Trait for S` and a generic one, without special-casing which
parts of a mixed impl block (some methods generic, some not — legal today,
since genericity is decided per-method, see Context) are which.

**Alternative considered:** give a trait method's own type parameters stable,
declaration-order-independent names (e.g., require the impl to redeclare the
identical parameter name the trait used) and compare by name instead of
position. Rejected — MAP-010 already made each declaration's type parameter
names purely local and cosmetic (two unrelated declarations can reuse `T`
freely; nothing after parsing carries the name forward except for
diagnostics), so requiring an impl to *echo* the trait's own spelling would
be a new naming-coupling rule with no precedent elsewhere in the language,
for no benefit over positional correspondence, which is also how Rust and
every other mainstream generic-trait implementation resolves this same
question.

### 3. A generic impl method's trait-reference validation and body are checked exactly like MAP-020 checks a generic free function, restarted at the `impl` boundary

Today, `collect()`'s `Item::Impl` branch resolves `trait_ref.name` against
`nominal.traits` and silently stores `None` on a miss (Context). This task
adds the two validations MAP-010 deferred here: unknown trait name (the same
"`{name}` は trait ではありません" diagnostic `check_impl` already uses) and
wrong trait-reference arity (`trait_ref.args.len()` against the newly-added
`TraitDecl.type_params.len()`, a new "trait `{name}` は型引数を {expected} 個
取りますが、{actual} 個渡しています" diagnostic, since no non-generic path
ever needed this message). Both run once per impl block, at the same point
`collect()` already builds `GenericOwner::Impl`, using `out.span =
Some(item.span())` — the same ambient-span pattern `check_impl`'s call site
already uses for a whole-impl diagnostic.

Body-checking mirrors MAP-020's `check_rigid` exactly, generalized from
"the declaration's own type parameters" to "the enclosing impl's type
parameters followed by the method's own" (already exactly what
`GenericDecl.type_params` holds for an impl method): each gets a synthetic
`#T<index>` `KnownType` name, the substituted signature is checked with
`Target::Discard`, and — because Decision 1 guarantees the impl's target is
already a concrete struct — the method's receiver binding is the *ordinary*,
already-concrete `receiver_type(mode, struct_name)` `check_body` already
builds for a non-generic impl method, not a synthetic name. This is strictly
additive: today's body-checking loop `continue`s over every generic impl
method (Context); this task replaces that `continue` with a call into the
shared rigid-checking function, so a never-called generic impl method with a
genuine type error is now reported, matching the parity MAP-020 already
established for generic free functions ("a bad generic body is reported even
if unused").

**Alternative considered:** skip rigid body-checking for impl methods in this
task, deferring it to whichever task first calls one. Rejected — leaving a
generic impl method's body completely unchecked until first use would mean a
`Map<T>`-shaped trait's impl could sit in a program with a broken body
indefinitely as long as nothing calls it yet, silently contradicting the
parity MAP-020 already set as the norm for "generic" anything in this
compiler, and would make this task's own test suite unable to exercise
"a type error in an uncalled generic impl body is reported" the same way
MAP-020's suite exercises it for a free function.

### 4. Method-call resolution falls back, only on a miss, to searching generic impls by receiver struct and inferring remaining type arguments from the arguments

`resolve()`'s `ExprKind::Field` arm keeps its existing behavior completely
unchanged through its existing `from_type`/`concrete_target` lookup. Only
when that lookup finds nothing (`Ok(None)`, today's existing "not found"
outcome) does a new step run: collect every `GenericDecl` with
`GenericOwner::Impl { target: GenericType::Struct(id), .. }` where `id`
equals the receiver's already-resolved struct and `name` (the method name)
matches. (Decision 1 guarantees `target` is always exactly this shape for
any impl this task accepts, so no unification is needed for this step —
it is a plain equality lookup, indexed by `(struct name, method name)` the
same way `Ids.methods` already indexes concrete methods.)

- **Zero candidates:** fall through to today's existing "呼び出し先が決まり
  ません" diagnostic, unchanged — from this arm's perspective a missing
  generic impl looks exactly like a missing non-generic one already does.
- **More than one candidate:** ambiguous — diagnose naming the receiver's
  type, the method name, and each competing candidate's trait name and
  declaration span, before inferring anything.
- **Exactly one candidate:** `synth` each call argument (unless already
  synthesized by the caller) and `unify()` them against the candidate's
  `GenericDecl.params`, exactly as MAP-020's `generic_call` does for a free
  function call — the same "unresolved"/"conflicting" diagnostics, the same
  message shapes, reusing the same `unify` function unchanged. Every one of
  the candidate's `type_params` must end up bound (Decision 1: none of them
  come from the receiver, all must come from the arguments), so this reuses
  `generic_call`'s existing "every declared type parameter must be bound"
  check verbatim.

A **duplicate impl** — two generic impl blocks (or one generic and one
non-generic) both providing the same trait for the same struct — is instead
diagnosed once, at declaration time, independent of any call site: alongside
`Ids.trait_impls`, this task adds tracking of every generic impl's
`(struct, trait)` pair span; a second impl block naming the same pair (either
generic or, via `Ids.trait_impls`'s existing entries, non-generic) is
rejected with a "`{trait}` は `{type}` に対して既に実装されています"
diagnostic pointing at both spans. This does not touch or introduce a check
for two *non-generic* impls of the same pair — `Ids.trait_impls`'s existing
silent-overwrite behavior for that case is unrelated to this task and stays
as it is.

**Alternative considered:** try the generic-impl search unconditionally,
before the existing non-generic lookup, and let ambiguity between a
non-generic and a generic impl providing the same method be diagnosed the
same way ambiguity between two generic impls is. Rejected — MAP-Q1 already
treats non-generic declarations as the stable baseline generics are added
alongside, not renegotiated against; searching only after a miss keeps
"non-generic trait/implの現行のmethod resolutionを保つ" exact (existing
programs take the identical code path they always have, with no new
candidate ever considered), and a program mixing a non-generic and a generic
impl of the same trait for the same struct is already rejected as a
duplicate impl at declaration time (this decision, previous paragraph)
before any call site's ambiguity check could even run.

### 5. A resolved call is monomorphized exactly like MAP-020 monomorphizes a generic free function call, reusing its cache and recursion-safety unchanged

Once a call resolves to exactly one candidate with every type argument
solved (Decision 4), building the concrete instantiation reuses MAP-020's
`instantiate()` unchanged in its caching and recursion-safety structure
(`Vec<(GenericFnId, Vec<hir::Type>, CallableId)>` cache checked first,
`Vec<(GenericFnId, Vec<hir::Type>)>` "currently instantiating" stack for
same-key sharing and different-key polymorphic-recursion rejection) — that
machinery is already keyed by `GenericFnId` alone and does not care whether
the declaration's owner is `Free` or `Impl`. The one difference is the shell
`hir::Callable`'s `owner`: instead of always `CallableOwner::Free`, it is
`CallableOwner::TraitImpl(id)`, where `id` is a `TraitImplId` allocated once
per `(struct, trait)` pair the first time any method of that pair is
resolved (a small new `Vec<((StructId, TraitId), TraitImplId)>` cache,
alongside the existing instantiation cache) and reused for every subsequent
call, regardless of that call's own method-level type arguments.

This lazily-allocated `TraitImplDecl`'s `methods` map is left empty. As
Context notes, it exists only for ambient/slot dispatch (`Call::Slot`), which
this task does not extend to generic impls (Non-Goals); a statically
resolved `Call::Method { callable, recv }` already carries the concrete
`callable` the compiler chose, and never looks the method back up through
`TraitImplDecl.methods`. Consequently this task does not need to solve how a
single trait method, called with two different sets of its own type
arguments in the same program, would occupy that map's one `TraitMethodId`
slot — a question the non-generic case never has to answer, since a
non-generic method has exactly one instantiation.

The receiver itself is handled exactly as it is for a non-generic method
call: `conform_receiver` (unchanged) validates the resolved candidate's
`receiver` mode against the synthesized receiver expression and inserts an
automatic shared borrow at `&self`, precisely as it already does today.

**Alternative considered:** widen `TraitImplDecl.methods` to
`BTreeMap<TraitMethodId, Vec<(Vec<hir::Type>, CallableId)>>` so every
distinct method-level instantiation is recorded there too, keeping the
non-generic and generic cases structurally uniform. Rejected — nothing reads
`TraitImplDecl.methods` for a statically-dispatched call, so populating it
would be dead bookkeeping; it would also force every non-generic construction
site (`lower_impl`, `link_trait_impls`) to wrap its one `CallableId` in a
one-element `Vec` for no behavioral gain, adding surface area to a type nine
other call sites already read, purely to describe a case (multiple
instantiations of the same trait method reachable through the map) that
literally cannot be observed until ambient dispatch of a generic trait method
exists, which is out of this task's scope.

## Risks / Trade-offs

- [Scoping contract-checking and resolution to struct-targeted impls
  (Decision 1) means this task cannot itself exercise `impl<T> Map<T> for
  [T]`] → Accepted; the spec and this task's own tests use a struct-targeted
  stand-in trait/impl to exercise the identical contract-checking and
  resolution logic `Map<T>` will need, and the proposal states plainly that
  the real array `impl` is MAP-080's job once MAP-075 exists. The
  positional-correspondence and substitution mechanism (Decision 2) does not
  depend on the target's shape, so extending it to a non-struct target later
  is a `CallableOwner`/monomorphization change, not a contract-checking
  rewrite.
- [Positional correspondence between a trait method's own type parameters and
  an impl method's own (Decision 2) silently accepts an impl that reorders
  them, e.g. implementing `fn m<U, V>(a: U, b: V)` as `fn m<V, U>(a: V, b:
  U)`] → This is not actually a mismatch: renaming a method's own type
  parameters is exactly as behavior-preserving as renaming a local variable,
  since nothing outside the declaration can observe the name (MAP-010).
  Reordering the *declared list* while also swapping every corresponding use
  in `a`/`b` produces an equally well-typed impl under this task's
  positional rule, and a genuine mismatch (using `U` where the trait's
  contract requires `V`'s substitution) is still caught because the
  shape comparison runs on the fully-substituted/-corresponded tree, not on
  the parameter list order alone.
- [The new duplicate-impl check (Decision 4) only fires when at least one
  side of a colliding `(struct, trait)` pair is generic-touching, leaving
  two purely non-generic impls of the same pair to keep silently
  overwriting each other in `Ids.trait_impls`, as today] → Accepted as an
  explicit non-goal; fixing that pre-existing, unrelated gap is not part of
  "generic trait/implの契約検査とmethod resolution" and risks changing an
  existing program's diagnostics outside this task's stated boundary
  ("non-genericの現行のmethod resolutionを保つ").
- [Reusing MAP-020's instantiation cache/stack for both free functions and
  impl methods without separating them could let a future compiler change
  in one accidentally regress the other] → The cache is keyed by
  `GenericFnId`, which is already unique per declaration regardless of
  owner, so no key can collide across the two cases; a focused test asserts
  a free-function instantiation and an impl-method instantiation with the
  same type arguments do not share a cache entry.

## Migration Plan

1. Add `hir::TraitDecl.type_params` and `hir::substitute_generic`, with
   focused `hir.rs` tests mirroring the existing `substitute`/`is_concrete`
   test style, behind the existing full test suite (zero behavior change for
   any non-generic program, since both are purely additive).
2. Add generic impl trait-reference validation (unknown trait name, arity
   mismatch) and the duplicate-impl-pair check at declaration time, with
   focused `typecheck.rs` tests for each diagnostic and its span.
3. Add contract-checking (substitution, positional correspondence, shape
   comparison, missing/mismatched-method diagnostics), with focused tests
   covering a matching generic impl, a missing method, a duplicated method,
   and each of the three shape-mismatch messages (receiver/params/ret).
4. Replace body-checking's `continue` over generic impl methods with rigid
   whole-body checking, with focused tests mirroring MAP-020's generic-body
   test set (accepted body, a genuine type error reported despite no call,
   two of the impl's own type parameters not interchangeable).
5. Add method-call resolution's generic-impl fallback (candidate search,
   ambiguity, unification/inference, `Ids` indexing), with focused tests for
   a single-candidate success, zero-candidate fallthrough to the existing
   diagnostic, and multi-candidate ambiguity.
6. Add the monomorphization path (lazy `TraitImplId` allocation, reuse of
   `instantiate`'s cache/stack), with focused tests for sharing across
   repeat calls, distinct instantiations for distinct type arguments, and
   polymorphic recursion through an impl method.
7. Add `tests/cli.rs` diagnostic tests (missing/duplicate/ambiguous/
   mismatched-signature) asserting rendered text and span, and confirm the
   full existing test suite, `cargo fmt --check`, and warnings-as-errors
   Clippy stay green with no output change for any program that declares no
   generic trait or impl.
8. Sync `generic-type-parameters`'s delta spec in this change into
   `openspec/specs/generic-type-parameters/spec.md` and commit the verified,
   test-passing state as a single stable snapshot.
