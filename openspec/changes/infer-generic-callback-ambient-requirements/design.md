## Context

See `proposal.md` for motivation. This picks up where MAP-040
(`archive/2026-08-05-widen-generic-specialization-key`) left off — the
comment at its call site (`src/typecheck.rs:2280-2288`) says outright that
the widened `(GenericFnId, type_args, callback_key)` instantiation key exists
so a later task can "貼れる" (attach) a different ambient requirement per
instantiation, and names MAP-050 for it.

Two independent modules already implement callback-specialized ambient
inference for **non-generic** code, both predating the generic-`map`
milestone entirely:

- `src/requirement.rs`: `analyze_hir` walks every `hir::BodyId` reachable
  from `program.bodies`, seeded with an empty `hir::Bindings`
  (`BodyKey::Body(id, Bindings::new())`), and additionally walks
  `BodyKey::Body(callee, inner)` for every call site, where `inner =
  hir::callee_bindings(program, body, callee, args, bindings)` — a
  positional zip of the callee's own declared parameters against the call's
  actual arguments, keeping only the ones that statically resolve to a named
  function (`hir::callable_of`, `hir::resolve_bindings`). Two calls to the
  same physical `Callable` with different callback arguments produce two
  different `BodyKey`s and therefore two independently fixed-pointed
  `BodyReqs` entries in `Analysis.semantic: BTreeMap<(BodyId, Bindings),
  BodyReqs>`, retrievable exactly via `Analysis::requirements(body,
  bindings)`.
- `src/ambient_abi.rs`: `Planner::plan_call`/`Planner::request` do the same
  zip (`hir::callee_bindings`) to compute the `bindings` component of an
  `InstanceKey { body, bindings, providers }`, and `Plan::intern` shares an
  `Instance` exactly when the full key (body, bindings, *and* the providers
  actually available at that call site) matches, producing one `RecordLayout`
  and one set of `PlannedCall`s per distinct specialization.

Neither module has any conditional on whether `body`'s `Callable` came from a
generic instantiation or a hand-written declaration — both are driven purely
by the shape of the finished HIR (`Callable.params`, `Body`'s `Let`s and call
argument expressions), which MAP-020/025's monomorphization model
(`instantiate()` in `src/typecheck.rs`) already makes indistinguishable
between a generic instantiation and an ordinary function: "producing an
ordinary, fully concrete callable with no unbound type parameter anywhere in
its signature or body" (`generic-function-instantiation` spec, already
shipped). Given that, and given MAP-040 already makes `apply<int, int>` bound
to `fetch` and `apply<int, int>` bound to `sendEmail` two distinct
`CallableId`s *before* `requirement.rs`/`ambient_abi.rs` ever see them, both
modules' existing call-site edge (`hir::callee_bindings` applied to the
*caller's* args, which still literally mention `fetch` or `sendEmail` as a
`Function` expression or local alias, regardless of which generic
instantiation is being called) reconstructs the right per-instantiation
binding automatically. No feature is missing from the algorithm.

What is missing is verification and one small, concrete, already-identified
reporting bug (Decision 2 below), found by reading `analyze_hir`'s summary
construction against what `instantiate()` actually names an instantiation.

## Goals / Non-Goals

**Goals:**
- Prove, with tests that exercise the full `check_and_lower` →
  `ownership::check` → `requirement::analyze` (→ `ambient_abi::plan` for the
  ABI-planning half) pipeline, that a generic function's ambient requirements
  are inferred correctly and independently per `(type arguments, callback
  binding)` specialization, including through nested generic-to-generic
  callback forwarding, and that a callback-free specialization never
  acquires a sibling specialization's slots.
- Prove that a missing-provider diagnostic reaching a generic call site
  reports a path naming both the generic helper and the selected callback,
  the same shape the existing non-generic diagnostic already has.
- Fix `Analysis`'s display-name summary map so two reachable specializations
  of one generic declaration are both reported, instead of the second
  silently overwriting the first.

**Non-Goals:**
- Any change to `scan`/`scan_body`/`hir::callable_of`/`hir::callee_bindings`/
  `hir::resolve_bindings` (`requirement.rs`) or `Planner::walk`/`plan_call`/
  `request` (`ambient_abi.rs`). Decision 1 argues, and the new tests confirm,
  these already produce correct results for generic instantiations without
  modification.
- Propagating a requirement through `hir::Call::Method` (an ordinary
  value-receiver method call, as `xs.map(f)` will be once MAP-080 implements
  it). `scan`'s `Call::Method` arm deliberately does not add a requirement
  edge today — a documented, pre-existing conservative approximation
  (`src/requirement.rs`, the comment above the `Call::Method` match arm)
  that predates generics entirely and is not specific to them. MAP-080's own
  completion criteria explicitly own "callback の ambient 要求が `map` と
  trait dispatch を経由して正確に伝播する" (ROADMAP.md, MAP-080). Every
  scenario this change adds uses the `apply<T, U>`-shaped free-function call
  (`Call::Direct`/`Call::Indirect`) that MAP-020/025/040's own fixtures all
  use for this exact purpose, not a method-call-dispatched generic impl
  method.
- `ambient_abi::PlanError::show()`'s error text for a `MissingProvider`/
  `TypeOnlyProvider`/`UnimplementedMethod` reaching a specific generic
  instantiation: it renders `program.show_body(body)`, which has the same
  name-collision property as `requirement::Analysis`'s summary map. This path
  is reached only when an invariant the earlier `requirement::analyze`
  diagnostic pass should already have caught is violated (`ambient_abi.rs`'s
  own module doc: "計画できない状態…ここへ来るのは不変条件の破れ"), so it is
  not the primary user-facing "you forgot to provide X" diagnostic
  (`requirement::Analysis::unsatisfied_for`/`errors_for_roots` is), and
  `Plan::render()` — the one place a `Plan`'s contents are normally shown —
  already disambiguates every instance with a numeric `instance#N` prefix
  regardless of this. Left as-is.
- Any change to `src/typecheck.rs`, `src/ownership.rs`, the interpreter, or
  Wasm generation.

## Decisions

### 1. No production change to the call-site edge logic in `requirement.rs`/`ambient_abi.rs`; verify with tests instead

Tracing a concrete example end to end: source declares `fn apply<T, U>(f: fn(T
-> U), x: T -> U) { f(x) }` and `fn fetch(id: int -> User?) { db.find(id) }`,
and calls `apply(fetch, 1)` inside `main`.

1. Typechecking resolves the call, infers `T = int, U = User?`, and
   `instantiate()` allocates one `Callable` shell (say `#7`) for `(apply,
   [int, User?], [Some(fetch)])`, then lowers `main`'s call expression to
   `Call::Direct { callable: #7, args: [Function(fetch), Int(1)] }` — the
   argument list still literally contains a reference to `fetch`, because
   monomorphization only changes *which callable id* the call targets, never
   the argument expressions themselves.
2. `requirement::analyze_hir` reaches `#7` two ways: once from the
   unconditional top-level seed (`BodyKey::Body(#7, Bindings::new())` —
   `f(x)` is unresolvable with empty bindings, contributes nothing), and once
   via the edge `scan` adds when it walks `main`'s `Call::Direct` to `#7`:
   `inner = hir::callee_bindings(program, main_body, #7, [Function(fetch),
   Int(1)], {})`, which zips `#7`'s own declared params (`[f_param,
   x_param]`) against the args and resolves `args[0]` (`Function(fetch)`) via
   `hir::callable_of` to `Some(fetch)` — giving `inner = {f_param: fetch}`.
   The resulting `BodyKey::Body(#7, {f_param: fetch})` scan resolves `#7`'s
   own `f(x)` via those bindings to `fetch`, walks into `fetch`'s body, finds
   `db.find(id)`, and the `db` requirement flows back up through `#7` to
   `main` — mechanically identical to the existing non-generic
   `apply`/`ticked` test (`requirement.rs`'s
   `callback特殊化ごとに要求が分かれる`), because nothing in this chain
   inspects whether `#7` is a generic instantiation.
3. `ambient_abi::Planner` does the same zip in `plan_call` (`inner =
   hir::callee_bindings(program, body, callee, call_args(call), bindings)`)
   to build `#7`'s `InstanceKey`, so a distinct `Instance`/`RecordLayout` is
   produced per `(type_args, callback)` specialization reached, exactly as
   MAP-040's own code comment anticipated.

This holds for any depth of nesting: if `apply`'s own body forwarded `f`
into another generic call, the *forwarding* call's args (`[Local(f_param),
...]`) resolve via `hir::resolve_bindings`/`callable_of` using the bindings
`#7` itself was walked with — which already correctly seeded `f_param ->
fetch` per step 2 — so the nested edge's `inner` binding is `{next_param:
fetch}`, and the chain continues. No part of this depends on generic-ness;
it depends only on `hir::callee_bindings`/`callable_of` being given the
right `(body, bindings)` pair to start from, which the edge-walking in both
modules already supplies uniformly for every callable in the program.

**Alternative considered:** add an explicit "is this call's callee a generic
instantiation" branch somewhere in `scan`/`plan_call` to special-case
requirement propagation. Rejected — there is nothing to special-case; the
existing generic code path already produces the right answer, and adding a
branch that does the same thing the default path does would be dead
complexity with no behavioral difference, only a larger diff to review and
maintain.

### 2. Fix the display-name collision in `Analysis`'s summary map without touching `Callable.name` or adding a new HIR field

`analyze_hir`'s last stage (`src/requirement.rs`, building `Analysis.reqs: BTreeMap<String,
Reqs>` and `Analysis.order: Vec<String>`) iterates `program.bodies` in
allocation order and does:

```rust
let name = program.show_body(*id);
public.insert(name.clone(), summary_of(&semantic, *id)...);
order.push(name);
```

`program.show_body`/`show_callable` (`src/hir.rs:1006-1028`) returns a
`Free`-owned callable's plain `callable.name` — and `instantiate()`
(`src/typecheck.rs:2346`) sets a fresh instantiation shell's `name` field to
`decl.name.clone()` verbatim, unconditionally, for every instantiation of a
generic declaration. This is intentional and load-bearing elsewhere:
`Callable.name`'s own doc comment calls it "正準表示名" (a display name,
not an identity), and existing MAP-020/025/040 tests
(`typecheck.rs`'s `instances(program, name)`/`hir::Program::free_callable`)
match callables by exact declared name to count/find instantiations —
changing `.name` itself to encode type arguments or callback identity would
silently break that established, widely-used test contract for no benefit
this change needs.

The concrete effect: if `apply<int, int>` has two reachable instantiations
(different callbacks, or different type arguments), `analyze_hir`'s loop
calls `public.insert("apply", ...)` twice — a plain `BTreeMap` insert, not a
merge — so the second call's `Reqs` silently replaces the first's, and
`order` (a `Vec`, not deduplicated) ends up with two `"apply"` entries that
both look up the same (second) `Reqs` in `render()`. This is a genuine loss
of information in the CLI's "推論された要求" listing (`src/main.rs:308`),
distinct from — and not compensated by — `Analysis::requirements(body,
bindings)` (the exact `BodyId`-keyed lookup `ambient_abi::plan` and any
future interpreter/production-planning caller actually use, which has no
collision because it is keyed by the unique `BodyId`) or
`unsatisfied_for`/`errors_for_roots` (which only ever look up `main`/test
names, never a generic instantiation's name, so they are unaffected).

**Fix:** make the summary-map construction assign each physically distinct
`BodyId` its own key deterministically, instead of keying purely by display
name. Concretely: track how many bodies have already produced a given
`show_body` string in this same pass (a local `HashMap<String, u32>` counter,
consulted and incremented once per `program.bodies` entry, in the existing
deterministic iteration order), and when a name repeats, disambiguate the
*key* used for both `public.insert` and `order.push` with a trailing ` #N`
(`N` = 2, 3, ... in encounter order) — e.g. `"apply"`, `"apply #2"`. This:
- adds no new field to `hir::Callable` or any other HIR type (no data
  representation change);
- keeps `program.show_body`/`Callable.name` themselves untouched, so every
  existing name-based lookup elsewhere in the codebase is unaffected;
- is deterministic, because `program.bodies`' allocation order already is
  (MAP-020 Decision 7 / MAP-040 Decision 5's transitive determinism
  guarantee, unchanged by this task);
- is purely additive to `Analysis`'s public surface (an existing `String`
  key gains an occasional ` #N` suffix) — no existing non-generic test
  observes a collision today (a non-generic declaration has exactly one
  `Callable`), so no existing assertion changes.

**Alternative considered:** store the solved type arguments (and resolved
callback names) on the instantiation shell and render a Rust-generics-style
name like `apply<int, int>` for the summary map specifically (reusing the
`shown(&type_args)`-style formatting `instantiate()`'s own
polymorphic-recursion diagnostic already uses at
`src/typecheck.rs:2309-2320`). Rejected for this task: it still would not
fully disambiguate two instantiations that share both type arguments *and*
declared name but differ only in callback binding without *also* rendering
the callback, turning a one-line counter fix into a small new
display-formatting concern with its own set of decisions (how to spell a
callback in a summary line, whether to show it for every generic
instantiation or only on collision, etc.) that ROADMAP.md's "設計判断: 不要"
boundary for this task should not need. The occurrence-counter suffix fixes
the actual bug (nothing is silently dropped) with a mechanical, two-line
change; a richer display format can be layered on later without changing
`Analysis`'s shape if anyone wants nicer text.

### 3. No new spec capability

`named-function-values` already owns "the system infers ambient requirements
through a higher-order helper from the named callback selected at each call
site" as a capability-level requirement, phrased in terms of "a helper" and
"the named callback selected at each call site" — wording that does not
distinguish a generic instantiation from a hand-written function. This
change's spec delta clarifies that wording to say so explicitly and adds
generic-specific scenarios, rather than opening a new capability that would
just duplicate the same requirement text under a different name.
`generic-function-instantiation` gets no Requirement-level delta because
nothing about *how a call site solves type arguments and picks an
instantiation* changes — only its prose Non-Goals list, which is not part of
the delta-spec mechanism (see `proposal.md` Impact; MAP-040 handled the
Non-Goal it fulfilled the same way, as a post-archive manual edit per its
`tasks.md` §5, not a `MODIFIED Requirements` block).

## Risks / Trade-offs

- [The core claim of this change — "the existing algorithm already handles
  generics" — rests on reading, not on a test that existed before this
  change] → Mitigated by writing the test matrix *before* concluding no
  production fix is needed for Decision 1; if any scenario in the new test
  matrix fails, that is itself the signal a real code change belongs in this
  task after all, not a design defect discovered later.
- [The ` #N` suffix in `Analysis.reqs`/`order` is not a stable identifier —
  reordering unrelated declarations changes which instantiation gets which
  suffix] → Accepted; the existing `render()` output was never a stable,
  parseable identifier scheme (it is keyed by declaration name today, which
  is already not unique across modules using `pub use` re-export names in
  some paths), and nothing consumes `order`/`reqs` programmatically outside
  `render()`/tests of `render()`'s text.
- [Leaving `ambient_abi::PlanError::show()`'s name collision unfixed] →
  Accepted per Non-Goals; revisit only if MAP-060/070 hits a real (not
  invariant-violation) code path where a `PlanError` for a specific generic
  instantiation needs to be user-facing and disambiguated, which is not the
  case today (`PlanError` should be unreachable for any program that already
  passed `requirement::Analysis`'s own diagnostics).

## Migration Plan

1. Add the test matrix to `src/requirement.rs` (generic callback
   specialization, callback-free sibling stays clean, nested generic-to-
   generic callback forwarding, missing-provider path naming both the
   generic helper and the callback) and to `src/ambient_abi.rs` (matching
   `Instance`/`RecordLayout`/`PlannedCall` shape assertions through
   `plan_hir_for_test`). Confirm every scenario passes with zero production
   code changes, validating Decision 1.
2. Fix `analyze_hir`'s summary-map construction per Decision 2; add a
   regression test with two reachable instantiations of one generic
   declaration confirming both appear in `Analysis.render()`'s output with
   their own (distinct) requirement sets.
3. After archiving, hand-edit `openspec/specs/generic-function-instantiation/spec.md`'s
   `## Non-Goals` per `proposal.md`'s Impact section.
4. Update `ROADMAP.md`'s MAP-050 row to `done`.
