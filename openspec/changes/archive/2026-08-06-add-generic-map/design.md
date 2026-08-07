## Context

See `proposal.md` for motivation. MAP-Q3/Q3A/Q4 (ROADMAP.md Decisions) fixed
`Map<T>`'s observable contract; this document fixes how the four passes that
already handle generic `impl`s for a struct target (`src/typecheck.rs`'s
generic-impl contract-checking, method-call resolution, and instantiation;
`src/requirement.rs`'s ambient-requirement inference) extend to an array
target, and how `map`'s own body is written.

Three facts, verified against the current tree before writing this design,
drive every decision below:

1. **`GenericOwner::Impl.target` is already a `GenericType` that can spell
   `[T]`; the code that consumes it is what is struct-only.** The type
   itself (`hir.rs:409-415`) and its doc comment already use
   `impl<T> Map<T> for [T]` as the illustrative example. What actually
   rejects it is `check_generic_impls` (`typecheck.rs:1715-1725`,
   `match block.target.kind { Struct(_) => true, Poison => false, _ =>
   {report "not a struct"; false} }`) and `instance_owner`
   (`typecheck.rs:2405-2412`, `let GenericTypeKind::Struct(struct_) =
   target.kind else { return (Free, None, name) }`). `add-generic-trait-
   resolution`'s design (its own Decision 1, and Non-Goals) verified this
   same thing and deliberately scoped itself to struct targets, naming this
   task as the one that widens it.
2. **A method call on an array receiver never reaches the generic-impl
   lookup at all today, independent of (1).** `KnownType::name()`
   (`typecheck.rs:183-188`) returns `None` for `KnownKind::Array`, and
   `resolve()`'s call-resolution match (`typecheck.rs:3987-4004`) only tries
   `decls.ids.generic_impls.get(&(type_name, name))` inside the
   `Some(type_name) => ...` arm — the `None` arm (array, and callable
   receivers) falls straight to `Ok(None)` ("no such member"), after the
   `.clone()`/`.push()` builtin interceptions have already had first refusal
   on that receiver (`typecheck.rs:3963-3986`).
3. **`hir::Call::Method` already carries everything `map`'s dispatch needs;
   only `src/requirement.rs`'s treatment of it is incomplete.**
   `generic_method`/`generic_call` (`typecheck.rs:4115-4174`) do not
   switch on whether the receiver's type is a struct or an array — they take
   a candidate list, a receiver, and a diagnostic label, and resolve to
   `CallTarget::Method` pointing at the concretely-instantiated
   `CallableId`, the same shape a struct method call already produces
   (`hir::Call::Method { callable, recv, args }`, `hir.rs:845-849`). The one
   real gap downstream is `src/requirement.rs`'s `scan`: its `Call::Method`
   arm (`requirement.rs:292-296`) pushes only to `Facts.walks` (so the
   specialization is visited and its own requirements are computed) and
   never to `Facts.calls` (the requirement-edge list `merge_facts` folds
   into a caller's own facts) — a documented, pre-generics conservative
   approximation the surrounding comment (`requirement.rs:285-291`) already
   flags as fixable by turning it into a proper edge.

## Goals / Non-Goals

**Goals:**
- Make `impl<T> Map<T> for [T]` — specifically, a generic `impl` whose
  target is an array whose element is that `impl`'s own type parameter —
  reachable through the same contract-checking, resolution, and
  instantiation machinery a struct-targeted generic `impl` already uses,
  with the smallest widening that shape needs.
- Write `map`'s body in ordinary, type-checked Rhodolite source (per
  ROADMAP's explicit "コンパイラ組み込みにする必要はない"), using MAP-075's
  `push` and the existing array-literal/`for`-loop rules — no new HIR node,
  no raw buffer access.
- Make a callback's ambient requirement propagate through `xs.map(f)` to its
  caller, by closing `Call::Method`'s existing gap in `src/requirement.rs`
  generally (the fix is not `map`-specific — nothing distinguishes a
  generic-impl-dispatched `Call::Method` from an ordinary struct one at that
  point in the pipeline).

**Non-Goals:**
- Any impl target shape beyond `[T]` where `T` is the impl's own type
  parameter: a concrete-element array (`impl Foo for [int]`), a callable
  type, a builtin, or a blanket `impl<T> Trait<T> for T`. `check_generic_impls`
  keeps rejecting all of those exactly as it does today.
- A borrowed `map` (`&self` / `fn(&T -> U)`) — MAP-Q4 fixed this out.
- Explicit type-argument syntax, associated-type-constructor or higher-kinded
  generalization of `Map<T>` — MAP-Q2/Q3A fixed these out.
- Any change to `Push<T>`'s own contract, capacity growth, or OOM behavior
  (MAP-075, already `done`) — `map`'s body only calls it.
- New ownership rules. `map`'s instantiated body is checked by
  `src/ownership.rs`'s existing generic-instantiation and `for`/`push`
  handling with (expected) zero new code, the same property MAP-020
  established for generic free functions and MAP-075 established for `push`
  call sites; this design does not add an ownership decision, but
  implementation must confirm the expectation against the tree, the same
  way `add-array-push`'s design recorded a deviation when one surfaced.
- Widening `hir::Call::Method`'s requirement-edge fix to also thread through
  `Call::Slot` (ambient/`with`-dispatched calls) — that path already records
  a `Call` edge today (`requirement.rs:279-283`, `BodyKey::TraitMethod`);
  nothing about it is method-receiver-shaped and it is unaffected by this
  change.

## Decisions

### 1. `check_generic_impls` accepts an array target only when its element is exactly the impl's own type parameter, keyed by a canonical string independent of the parameter's spelling

`block.target.kind` (`typecheck.rs:1715`) currently must be
`GenericTypeKind::Struct(_)`. This adds a second accepted case:
`GenericTypeKind::Array(element)` where `*element ==
GenericTypeKind::Param(p)` for some `p` in the `impl`'s own type-parameter
list (`block.own_params`-worth of the front of `block.methods`' shared type
parameters — the same list `check_generic_contract` already has). A `[T]`
target where `T` is *not* the impl's own parameter (there is no other
spelling available today, since generic struct/enum do not exist and a
concrete element type can only come from a struct or builtin name) cannot
occur; the check exists to give a precise diagnostic if it somehow did,
and to keep the accepted shape exactly as narrow as MAP-Q1's flagship
example, not "any array."

The existing index `Ids.generic_impls: BTreeMap<(String, String),
Vec<GenericFnId>>` is reused unchanged in shape. `block.type_name`
(`impl_target_name(target)`, `typecheck.rs:1087-1091`) keeps its literal,
diagnostic-facing spelling (`"[T]"`, `"[U]"`, whatever the source wrote) for
every message `check_generic_impls` already produces (ambiguity, duplicate-
impl, contract mismatch — all keep reading naturally, e.g. "impl [T]::map").
Only the *index key* — used for the call-site lookup and the same-target-
same-trait duplicate check (`typecheck.rs:1749-1772`) — is normalized to a
fixed sentinel (e.g. `"[]"`) whenever the target is this accepted array
shape, computed locally inside `check_generic_impls`/its dup-tracking
`pairs` map, not by changing `impl_target_name` itself (which stays used
as-is for non-generic `impl`s and for diagnostic text, where the literal
spelling is more readable and where widening its behavior would be an
unrequested, unrelated change to how `impl Foo for [int]`'s "not a struct"
diagnostic reads today).

**Alternative considered:** accept *any* array target, regardless of
whether its element is the impl's own parameter (i.e., treat `[T]` and a
hypothetical concrete `[int]` the same way once arrays are accepted at
all). Rejected — nothing in ROADMAP asks for `impl Foo for [int]` to work,
non-generic array impls have their own unrelated struct-only gate
(`check_impl`, `typecheck.rs:2652-2660`) this change does not touch, and
accepting it here would be new, unrequested surface with no design/spec
sign-off of its own.

**Alternative considered:** add a dedicated `Ids.array_impls: BTreeMap<String,
Vec<GenericFnId>>` keyed only by method name, instead of reusing
`generic_impls` with a sentinel key. Rejected as needless duplication —
`generic_impls`'s `(String, String)` key already has room for a second axis
that only ever takes one value ("is an array") without changing its type,
and every downstream consumer (`generic_method`, the duplicate-impl check)
already operates on strings and needs no new branch to handle a sentinel
value instead of a real type name.

### 2. Method-call resolution gets one new array-receiver arm in `resolve()`, after the existing builtin interceptions

`resolve()`'s `Field(recv, name)` match (`typecheck.rs:3961-4026`) already
tries `.clone()` then `.push()` before falling into the
`ty.name().filter(...)`-keyed struct path. This adds one more arm, tried
after `.push()` and before the `Some(type_name)`/`None` split: when the
receiver's checked type is an array (`ty.element().is_some() &&
!ty.optional`) and the call is not `.push` (already claimed above), look up
`decls.ids.generic_impls.get(&("[]".to_string(), name.to_string()))` — the
same sentinel Decision 1 writes at declaration time — and, on a hit, call
`generic_method` exactly as the struct path does, with `"[]"` (or a fixed
display string such as `"[T]"`) as the diagnostic `type_name`. A miss falls
through to the existing "no such member" outcome arrays already produce
today (`Ok(None)`), unchanged.

This keeps `generic_method`/`generic_call` (Context fact 3) completely
untouched: they already only need a candidate list, a receiver, and a
label, and do not care whether the underlying target is a struct or an
array.

**Implementation deviation (diagnostic label only):** the arm passes the
*receiver's own owned spelling* (`[int]`, computed by stripping the
receiver type's borrow the same way `KnownType::name()` looks through one
for a struct) as the diagnostic `type_name`, instead of the literal
sentinel `"[]"`. The sentinel stays confined to the index key, exactly as
Decision 1 requires; using it in messages would print `` `[]` の `map` が
どの trait のものか決まりません `` and `` `&[]::map` のレシーバは… ``,
which name no type the user wrote. Nothing else changes: the lookup key,
the candidate list, and `generic_method`/`generic_call` are as planned.

**Alternative considered:** make `KnownType::name()` return `Some("[]")`
for an array so the existing `Some(type_name) => ...` arm handles it with
no new match arm. Rejected — `name()`'s callers outside this one match arm
(`declares_member`, `declares_clone`, `from_type`, field-access resolution)
all assume a `Some` name means "a nominal type with a field/method table to
look up by that literal name," and would need their own array-awareness
audit to avoid a silent new behavior (e.g. field access on an array
resolving through the struct-field-lookup path with a bogus name). A
dedicated arm keeps the widening legible as "one new, narrowly-triggered
case," matching this repo's established precedent (`.clone()`, `.push()`)
for how a builtin-shaped receiver gets its own resolution step instead of
a change to the general nominal-lookup helper.

### 3. `instance_owner` substitutes the target through the caller's already-solved bindings; the array case gets `CallableOwner::Free`

`instance_owner(id, out)` (`typecheck.rs:2398-2442`) is called before its
caller computes `subst`/`bindings` (`typecheck.rs:2340-2345`), and today
only needs a fixed struct name for `self`'s type — a struct target never
varies with the `impl`'s type arguments (no generic struct/enum, MAP-Q1).
An array target's `self` type *does* vary (`self: [int]` vs. `self:
[User]` for two instantiations of the same `impl<T> Map<T> for [T]`), so
this reorders the split: `instance_owner` returns the target's shape and
the declared receiver mode without yet building the final `KnownType`;
the caller builds `self`'s `KnownType` uniformly for both target kinds by
substituting the (struct-name-or-array-element) `GenericType` through the
already-in-scope `bindings` the same way it already builds `ret`
(`known_generic(&declared_ret, bindings, &out.lowered)`,
`typecheck.rs:2361`) and overlays the declared receiver mode
(owned/`&`/`&mut`) the way `receiver_type` does today
(`typecheck.rs:244-253`). For a struct target this substitution is a no-op
(the target has no type-parameter-dependent part), so the struct path's
resulting `KnownType` and `CallableOwner::TraitImpl`/`Inherent` choice are
unchanged byte-for-byte.

**Implementation note (two further sites the same substitution reaches,
both found by the existing suite, neither a new decision):** the same
"`self`'s type is the target substituted through this context's bindings"
rule has to be applied at the two *other* places that spelled `self`'s type
from a fixed name, or `impl<T> Map<T> for [T]` cannot type-check at all.
(a) The rigid one-shot body check (`check_rigid`, `typecheck.rs`) built the
receiver from `impl_target_name(target)` — the literal string `"[T]"` as a
*nominal* name — so `for x in move self` reported "`for` の反復対象は配列
である必要がありますが、`[T]` です". It now takes the target from the
decl's `GenericOwner::Impl` and substitutes the rigid bindings, giving
`[#T0]`; a struct target still yields exactly the struct's own name, so the
existing rigid checks are unchanged. (b) The call site's receiver
conformance (`conform_receiver`, reached from `generic_call`) built the
expected type with `receiver_type(mode, type_name)`, i.e. a nominal name
again. It now takes the owned self type as a parameter — `plain(type_name)`
on the unchanged non-generic path, and the substituted target from
`generic_call` — with `type_name` kept for the diagnostic label only. Both
sites go through one new helper, `as_receiver(mode, owned)`, which is
literally the body `receiver_type` already had.

For the array case, `CallableOwner` is `Free` — mirroring today's dead
fallback branch instance_owner already has for a non-struct target
(`typecheck.rs:2406,2411`, previously unreachable because Decision 1 never
let this point be reached for an array; now the live, intended outcome).
There is no `StructId` an array target could supply for
`CallableOwner::TraitImpl`/`hir::TraitImplDecl.type_` (`StructId`-typed,
`hir.rs:560`, also read for ambient-provider naming in `ambient_abi.rs` and
`wasm.rs`) to reference, and `map` is never dispatched through `with`/an
ambient slot (MAP-Q3: `xs.map(f)` resolves at compile time to the impl,
never through slot machinery), so `TraitImplDecl` needs no widening at all
— `Free` gives `map`'s instances the same display/requirement-listing
treatment (`show_callable`'s `Free` branch, the `#N`-suffixed disambiguation
`requirement.rs`'s summary-map construction already applies per
`infer-generic-callback-ambient-requirements`'s fix) a generic free
function's specializations already get, and `provider()`
(`typecheck.rs:4179-4190`) already names an ambiguity candidate by its
trait (`Map`) regardless of `CallableOwner`, so ambiguity diagnostics are
unaffected either way.

**Alternative considered:** widen `hir::TraitImplDecl.type_`/
`CallableOwner::TraitImpl` to an enum over `StructId` or a concrete element
`hir::Type`, so an array impl gets a "real" `TraitImplDecl` the way a struct
impl does. Rejected — nothing consumes `TraitImplDecl` for `map` (no slot
dispatch, no `Push<T>`-style ambient naming need), it would force every
`program.structs[program.trait_impls[..].type_]` read in `ambient_abi.rs`/
`wasm.rs` (all naming-only, all currently struct-safe by construction) to
grow a match arm for a shape they can never actually observe today, and it
would resurrect the exact "how do ownership/interpreter/Wasm name and
dispatch an array-receiver method" question `add-generic-trait-resolution`'s
design deliberately deferred outward rather than a side effect of widening
a type it happens to reuse. `Free` answers that question with "the same
way a generic free function's instantiation already is," which is already
correct and requires no new representation.

### 4. `map`'s body is ordinary Rhodolite source using `push`, `for x in move self`, and a locally-declared `[U]`

```rhodolite
trait Map<T> {
    fn map<U>(self, f: fn(T -> U) -> [U])
}

impl<T> Map<T> for [T] {
    fn map<U>(self, f: fn(T -> U) -> [U]) {
        let mut result: [U] = []
        for x in move self {
            result.push(f(move x))
        }
        move result
    }
}
```

Every piece here is already-specified, existing behavior once Decisions 1-3
make the `impl` reachable: `let mut result: [U] = []` types the empty
literal against its declared expected type (`array-type-checking`'s "空の
`[]` はこの経路で任意の期待配列型に収まる"); `for x in move self` consumes
the receiver and binds each element owned, left to right
(`array-type-checking`'s `for x in move xs` rule); `f(move x)` is an
ordinary `Call::Indirect` through the parameter-bound callable value
(`named-function-values`), whose own ambient requirement already propagates
to `map`'s specialization via existing, unmodified `requirement.rs`/
`ambient_abi.rs` machinery (`infer-generic-callback-ambient-requirements`);
`result.push(...)` is MAP-075's builtin, auto-borrowing `result` exclusively
with no call-site `&mut`; `move result` returns ownership out. No new
grammar, no new HIR node, no compiler-synthesized body.

**Alternative considered:** give `map`'s body a compiler-synthesized
implementation the way `Push<T>::push` has one (Decision 1 of
`add-array-push`). Rejected outright — ROADMAP's own completion condition
for MAP-080 says the opposite of MAP-Q6's `push`: "`impl<T> Map<T> for [T]`
の本体を通常の Rhodolite コードで記述する." A synthesized body would also
contradict this task's own point: proving `push` (MAP-075) is sufficient
for ordinary source to build an array result, rather than needing another
compiler-only escape hatch.

### 5. `src/requirement.rs`'s `scan` treats `Call::Method` as a real requirement edge, mirroring `Call::Indirect`

`scan`'s `Call::Method { callable, args, .. }` arm changes from `out.walks
.push(BodyKey::Body(hir::BodyId::Callable(*callable), inner))`
(`requirement.rs:294-295`) to `out.calls.push(Call { callee:
BodyKey::Body(hir::BodyId::Callable(*callable), inner), provided:
provided.clone(), span: expr.span })` — dropping the walks-only push in
favor of the `Call` push, the same shape `Call::Indirect`'s arm already has
four lines above it (`requirement.rs:255-263`). `merge_facts`
(`requirement.rs:417-429`) already `extend`s `into.calls` from `from.calls`
unconditionally, so no change to merging itself is needed — the existing
generic fold already does the right thing once `Call::Method` starts
populating `calls` instead of only `walks`. `analyze_hir`'s reachability
queue (`requirement.rs:449-466`) already draws from both `calls` and
`walks`, so reachability is unaffected; only requirement *propagation*
changes for method calls.

This is not narrowed to generic-impl-dispatched calls: `hir::Call::Method`
does not distinguish "resolved to a struct's own method" from "resolved to
a generic array impl's instantiation" at this point in the pipeline (both
are already-resolved `{ callable, recv, args }` — Context fact 3), and
`scan`'s existing comment already frames the gap as general, not
generics-specific ("値レシーバのメソッド呼び出しは辺にしない…AST を歩いて
いた頃と同じ保守的な過小近似," `requirement.rs:285-286`). This also closes
a pre-existing, previously-undetected gap for an ordinary non-generic
method: a `&mut self`/`self` method whose body calls an ambient slot
through a callback parameter it invokes did not, before this change,
propagate that requirement to its own caller either — `infer-generic-
callback-ambient-requirements`'s Non-Goals explicitly named this as
pre-existing and out of its own scope, assigning it here.

**Alternative considered:** scope the fix narrowly to only generic-impl-
dispatched `Call::Method`s (e.g., by checking `program.callables[*callable]
.owner` for a marker), leaving an ordinary struct method's `Call::Method`
on the walks-only path. Rejected — there is no data available at `scan`'s
call site to distinguish the two cases without inventing one (`CallableOwner`
does not currently record "instantiated from a generic impl" vs. "declared
directly" — MAP-025 generic-impl instantiations reuse ordinary
`hir::Callable` on purpose, Context fact 3), and inventing that distinction
only to keep an artificial restriction nothing in ROADMAP asks for would be
new, unrequested machinery. The uniform fix is both the smaller diff and
the more correct one.

## Risks / Trade-offs

- [Turning on `Call::Method`'s requirement edge changes inferred-requirement
  results for every existing struct method call whose body invokes a
  callback parameter that needs an ambient slot, not just `map`'s new call
  sites] → Accepted as a correctness fix, not a behavior change users should
  be relying on: the prior behavior was an under-approximation the pipeline
  already documented as provisional, and this repo's `infer-generic-
  callback-ambient-requirements` design already flagged it as this task's
  job to close. Existing differential/requirement snapshot tests will show
  the widened edge if any covered program exercises the previously-missed
  path; none are expected to newly regress since no such program can invoke
  a struct method with a callback-shaped parameter today with a matching
  `with` provision expected to be absent under the old under-approximation
  (a missing-provider diagnostic can only get *more* correct, not less).
- [The `[]` sentinel key in `Ids.generic_impls` (Decision 1) is an implicit
  convention future readers must know cannot collide with a real struct
  name] → Accepted; struct names are declared identifiers (cannot contain
  `[`/`]`), so the sentinel is unambiguous by construction, and Decision 1
  documents it at the one site that both writes and reads it.
- [Reordering `instance_owner`'s split (Decision 3) touches the one existing,
  working struct-impl instantiation path, not just the new array path] →
  Accepted; the design requires the struct case's substitution to be a
  provable no-op (no type-parameter-dependent part in a struct target), and
  the migration plan runs the full existing generic-trait-resolution test
  suite unmodified before adding any new array-target test, to catch a
  byte-for-byte regression immediately if the no-op claim is wrong.

## Migration Plan

1. Add `Map<T>`'s trait declaration reachability (already-existing
   `GenericOwner::Trait` machinery; confirm with a focused test that mirrors
   `Push<T>`'s "declares its method" scenario) and `impl<T> Map<T> for [T]`'s
   parse/lower path without yet widening any check (expect a "not a struct"
   diagnostic at this step, proving nothing regressed silently).
2. Implement Decision 1 (`check_generic_impls`'s array-target acceptance and
   sentinel-keyed index) and Decision 2 (`resolve()`'s array-receiver
   lookup arm); add focused typecheck tests: the impl is accepted, an
   unrelated `impl Foo for [int]` (non-`T`-element or non-generic) still
   fails, a second `impl<T> Map<T> for [T]` is rejected as a duplicate, and
   `xs.map(f)`'s type argument (`U`) infers from the callback argument.
3. Implement Decision 3 (`instance_owner`'s substituted self-type, `Free`
   ownership); run the existing generic-trait-resolution struct-impl test
   suite unmodified to confirm zero regression, then add a focused test that
   two `map` instantiations over different element types each get their own
   correctly-typed `self` and distinct instantiation (per
   `generic-function-instantiation`'s existing caching contract).
4. Write `map`'s body (Decision 4) using MAP-075's `push`; add focused eval
   tests for `Copy`, non-`Copy`, and empty-array element cases, and for
   left-to-right callback application order.
5. Implement Decision 5 (`requirement.rs`'s `Call::Method` edge); add a
   focused requirement-inference test that a callback's ambient requirement
   reaches `xs.map(f)`'s caller, and a missing-provider test that the
   reachability path names `map`.
6. Add differential fixtures (`Copy` element, owned element, empty array,
   ambient-requiring callback) and register them in `FIXTURES`; run the full
   differential suite.
7. Sync the `generic-map` (new), `generic-trait-resolution` (modified), and
   `differential-execution` (modified) delta specs.
8. Run the full test suite, `cargo fmt --check`, and Clippy with warnings as
   errors; confirm two consecutive builds of each new fixture are
   byte-identical (ROADMAP's MAP-010-MAP-100 common completion condition);
   commit the stable, passing state as a single snapshot.
9. Mark MAP-080 `done` in `ROADMAP.md`.
10. Rollback is a plain revert: this change widens three existing,
    already-generic passes to one additional target shape and fixes one
    documented under-approximation — no public ABI, existing data
    representation, or struct-targeted behavior changes (Risk 3's no-op
    claim is verified by step 3's unmodified-suite run before any new code
    lands on top of it).
