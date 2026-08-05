## Context

See `proposal.md` for motivation. This picks up exactly where MAP-020
(`archive/2026-08-05-add-generic-function-instantiation`) and MAP-025
(`archive/2026-08-05-add-generic-trait-resolution`) left off, both of which
explicitly deferred widening the specialization key to MAP-040.

Today, `instantiate()` (`src/typecheck.rs:2259`) caches an instantiation in
`Out.instances: Vec<(hir::GenericFnId, Vec<hir::Type>, hir::CallableId)>` and
guards recursion with `Out.building: Vec<(hir::GenericFnId, Vec<hir::Type>)>`.
`generic_call()` (`src/typecheck.rs:2110`) infers `type_args` from the call's
already-`synth`-checked arguments and passes them to `instantiate()`. Neither
function looks at *which* concrete function is bound to a callable-typed
parameter — MAP-020 Decision 6 named this gap explicitly and MAP-025 Decision
5 reused the same cache unchanged for generic impl methods (keyed by
`GenericFnId` alone, which already distinguishes a free function from an impl
method and one impl target from another — MAP-020's original Non-Goals
wording named "trait implementation choice" as a second axis MAP-040 might
need to add, but MAP-025's own Non-Goals, written after trait resolution
existed, narrows this to callback identity only; a statically resolved
`Call::Method` already carries the concrete `CallableId` MAP-025's resolution
chose, so there is no remaining "which impl" ambiguity for this task to
resolve).

Callback identity itself is not a new concept in this compiler — it already
has a full, tested representation, just not one `instantiate()` consults:

- `hir::Bindings = BTreeMap<LocalId, CallableId>` (`src/hir.rs:1564`): within
  one body, which immutable local (if any) is bound to which named function.
- `hir::callable_of(body, expr_id, bindings)`: what a single expression
  statically refers to as a callable value (a `Function(CallableId)` literal,
  or a `Local` present in `bindings`).
- `hir::resolve_bindings(body, params)`: walks a body's `Let`s in allocation
  order (which is source order) extending `params` with every immutable
  local alias of a callable value — `let g = f` makes `g` resolve the same
  way `f` does.
- `hir::callee_bindings(program, body, callee, args, bindings)`: for a call,
  zips the callee's own declared parameters (`Callable.params: Vec<LocalId>`)
  against the call's arguments, keeping only the callable-valued ones — this
  is exactly a "callback key" already, just computed post-hoc.

These four functions are used today only downstream of type-checking, by
`requirement.rs`'s ambient-requirement analysis (keyed by `(BodyId,
Bindings)`) and `ambient_abi.rs`'s ABI-instance planning. For a *non-generic*
function, this is sufficient on its own: `apply` exists as exactly one
`hir::Callable`, and `requirement.rs` re-derives a different `(BodyId,
Bindings)` key per distinct call site's callback binding by walking the
already-built HIR, without needing separate physical `hir::Callable`s.

That does not carry over to a *generic* function, because MAP-020's
monomorphization model produces one **physical** `hir::Callable` per
distinct `instantiate()` key — today, per `(GenericFnId, type_args)` — by
re-running `check_body` against substituted types. If two calls to
`apply<int, int>` with different callback bindings shared that one physical
instantiation (as they do today), MAP-070 (Core Wasm generation, a later
task) would have exactly one Wasm function to emit for both — but MAP-050
(the task right after this one) needs each distinct callback binding to
carry its own ambient-requirement set, which must show up as a different set
of ambient/slot parameters on the compiled function for `apply(double, ...)`
than for `apply(sendEmail, ...)`. A single Wasm function cannot have two
different ABIs depending on which caller reached it, so the callback
dimension has to already be a separate physical instantiation by the time
MAP-070 compiles it — which is exactly what this task's widened
`instantiate()` key produces, ahead of when MAP-050/070 need it (matching
MAP-020 Decision 6's stated reasoning for why the narrower key was safe only
temporarily).

## Goals / Non-Goals

**Goals:**
- Widen `instantiate()`'s cache and recursion-stack keys to
  `(GenericFnId, type_args, callback_key)`, where `callback_key` is a
  per-declared-parameter `Vec<Option<hir::CallableId>>` resolved the same
  way `hir::callable_of`/`hir::resolve_bindings` already resolve callback
  identity for non-generic code.
- Keep same-key recursion sharing a single reserved instantiation, exactly
  as today, just under the wider key.
- Keep polymorphic-recursion detection scoped to type-argument divergence
  only, and show why callback-binding divergence cannot occur across
  recursion depth given this language's existing callable-value constraints
  (no new diagnostic needed for it).
- Keep instantiation strictly call-site-driven (lazy): no eager
  cross-product of type arguments and callback bindings, no new
  reachability pass — this already falls out of `check_and_lower`'s single,
  deterministic, call-driven pass (MAP-020 Decision 7; also stated as a
  Non-Goal in both existing specs' text about MAP-040).

**Non-Goals:**
- Ambient/effect requirement inference through a callback-typed parameter on
  a per-callback basis — MAP-050. This task only makes the physical
  instantiations MAP-050 needs to attach different requirement sets to; it
  does not compute or attach those sets itself.
- Any change to `requirement.rs`/`ambient_abi.rs`'s own `(BodyId, Bindings)`
  keying, which already works correctly for non-generic code and continues
  to work unchanged for generic instantiations (each is just one more
  distinct `BodyId` now, exactly like any other callable).
- Any change to ownership checking, the interpreter, or Wasm generation —
  MAP-030 already established that ownership checking never looks at
  `CallableOwner` or instantiation identity, and a widened-key instantiation
  is still an ordinary `hir::Callable` to every downstream pass.
- A new diagnostic for "callback-binding polymorphic recursion" — see
  Decision 4.

## Decisions

### 1. `callback_key` is computed at the call site from the caller's own partially-built body, reusing `hir::resolve_bindings`/`callable_of` rather than new incremental tracking

`check_and_lower`'s single-pass checker keeps the body currently under
construction in `Out.body: hir::Body` (`src/typecheck.rs:2921`), swapped in
and out around each `check_body` call. By the time `generic_call` processes
a call expression, every earlier expression in that same body — including
every earlier `let` — is already allocated into `out.body`, in source order.

This means `generic_call` can compute the caller's *full* current binding
table on demand, by calling the existing `hir::resolve_bindings(&out.body,
seed)` right there, rather than threading a new incrementally-updated
binding map through the whole body-checking hot path. `seed` is a new
`Out.callback_bindings: hir::Bindings` field, empty for every ordinary
(non-generic-instantiation) `check_body` call, and populated (Decision 2)
only while checking a generic instantiation whose own declared parameters
were themselves bound to a callback by the call that triggered it.

For each of a call's already-`synth`-checked arguments (`checked[i].id`),
`callback_key[i] = hir::callable_of(&out.body, checked[i].id,
&hir::resolve_bindings(&out.body, &out.callback_bindings))`. This is
computed uniformly across all argument positions, not just declared
callable-typed ones — a non-callable-typed argument's expression can never
structurally match `callable_of`'s `Function`/bindings-hit arms, so it
always yields `None` without needing to special-case it.

**Alternative considered:** thread a live, incrementally-updated
`hir::Bindings` through `Cx`/`Locals`, inserting at each `Let` exactly as
`resolve_bindings` does, instead of recomputing on demand at each generic
call site. Rejected as unnecessary — `resolve_bindings` is already a full
forward scan and bodies in this language are small; recomputing it at each
generic call site (there are far fewer generic call sites than expressions)
avoids adding a new field to `Cx`/`Locals` and a new update site to the
`Let`-handling code that every non-generic body-check would also pay for.

### 2. A generic instantiation's own callback-typed parameters are seeded into `Out.callback_bindings` right after their `LocalId`s are allocated

`check_body` (`src/typecheck.rs:2903`) allocates each declared parameter's
`LocalId` in a loop (lines 2930-2937) before checking the body. This loop
gains one more input: `callback_key: &[Option<hir::CallableId>]`, positional
and same length as `params`, defaulting to `&[]`-equivalent (all `None`) for
the three existing non-generic call sites (free fn, impl method, test) —
mechanically the same "existing call sites pass the empty/default case"
shape MAP-020 Decision 1 already used for `Cx.type_params`. Immediately
after allocating parameter `i`'s `LocalId`, if `callback_key[i]` is
`Some(callable)`, the pair `(new_local_id, callable)` is inserted into a
fresh `hir::Bindings` that becomes `out.callback_bindings` (saved/restored
around the call exactly like `out.body` already is) for the duration of
checking that body. This is what lets a nested generic call inside, say,
`apply`'s own instantiated body resolve `f`'s identity: `out.callback_bindings
= {f_local: double}`, and `hir::resolve_bindings` (Decision 1) extends that
with any further `let` aliases before the nested call's own `callback_key`
is computed.

`instantiate()` computes this `callback_key` once (Decision 3) before
allocating the shell, and passes it into `check_body` alongside the already
existing substituted parameter/return types.

**Alternative considered:** resolve callback identity for a generic
instantiation's nested calls in a post-pass over the finished HIR, the way
`requirement.rs` already does for non-generic code, rather than seeding it
during checking. Rejected — `instantiate()` needs `callback_key` *before*
recursing into the body (to place the reserved cache/recursion-stack entry
ahead of the recursive call, per Decision 3/MAP-020 Decision 5), and a
post-pass cannot influence which physical `hir::Callable` monomorphization
already built.

### 3. `Out.instances`/`Out.building` key on `(GenericFnId, type_args, callback_key)`; polymorphic-recursion detection still scans by `GenericFnId` and type arguments only

`Out.instances` becomes `Vec<(hir::GenericFnId, Vec<hir::Type>,
Vec<Option<hir::CallableId>>, hir::CallableId)>` and `Out.building` becomes
`Vec<(hir::GenericFnId, Vec<hir::Type>, Vec<Option<hir::CallableId>>)>`.
`instantiate()`'s cache lookup and "reserve the slot before recursing" logic
(`src/typecheck.rs:2266-2327`) compare the full triple, unchanged in
structure from today — this directly gives same-key sharing and
same-key-recursion termination under the wider key (spec: "A solved call
site produces a cached, concrete instantiation" and "Self-recursive
instantiation... does not loop").

The polymorphic-recursion scan (`out.building.iter().find(|(generic, args)|
*generic == id && *args != type_args)`) keeps comparing only `GenericFnId`
and `type_args`, ignoring `callback_key` — so a `Out.building` entry with the
same `id`/`type_args` but a different `callback_key` is not flagged, and
does not block a legitimate different-callback instantiation from starting
(that case does not go through the polymorphic-recursion branch at all: it
is a cache *miss* on the full key, so it proceeds to build a new
instantiation exactly like any other cache miss — it just happens to share a
`building`-stack entry's `id`/`type_args` without being that entry).

### 4. Callback-binding divergence across a recursive cycle is not diagnosed, because it cannot happen

The polymorphic-recursion diagnostic exists because *type* arguments can
grow without bound across a recursive cycle (`grow([value], n - 1)` nests
one more array layer each call). Callback bindings cannot: this language's
existing constraint (`src/hir.rs:1561`, predating MAP-series) is that a
callable value is always either a direct reference to a declared function
(`ExprKind::Function`) or an immutable local alias of one — there is no
expression form that constructs a *new*, different callable value at
runtime (no closures, no computed function values). Consequently, a given
call expression's callback argument is a fixed piece of syntax: forwarding a
generic function's own callback-typed parameter to a recursive call
resolves to whatever that parameter was itself bound to at the call that
triggered the current instantiation (Decision 2) and stays that value at
every deeper level; passing a literal named function instead is fixed to
that one function at every level. Either way, the `callback_key` computed
for one static recursive call site cannot differ between its first and any
later evaluation during a single `instantiate()` call chain, so it cannot
grow the way type arguments can. No new diagnostic is needed to keep this
finite. The `generic-function-instantiation` spec's polymorphic-recursion
requirement states this explicitly so it is a documented, tested boundary
rather than a silent assumption.

### 5. Determinism is unaffected

`callback_key` is built from `hir::CallableId`s already allocated
deterministically (arena order) and from a `BTreeMap`-backed
`hir::Bindings`, walked in the same deterministic, single-threaded,
source-order pass MAP-020 Decision 7 already relies on for `type_args` and
the rest of the compiler's determinism. No hashing or unordered iteration is
introduced.

## Risks / Trade-offs

- [Recomputing `hir::resolve_bindings` at every generic call site (Decision
  1) instead of tracking bindings incrementally] → Accepted; generic call
  sites are a small fraction of a body's expressions, and this avoids adding
  a hot-path field update to every `Let`. Revisit only if a pathological
  program with very large bodies and many generic calls makes this
  measurably slow, which nothing in this task's scope exercises.
- [Widening `Out.instances`/`Out.building`'s key touches the exact tuple
  shape MAP-025 Decision 5 said was shared, unmodified, between free
  functions and impl methods] → The sharing itself (one cache, one stack,
  regardless of `CallableOwner`) is preserved; only the tuple's arity grows,
  mechanically, for both owners at once — MAP-025's own boundary (owner is
  chosen by `instance_owner`, not by the cache) is untouched.
- [A program that never passes a callback argument to a generic function
  sees no behavior change at all] → Intentional; `callback_key` is all
  `None` for such calls, so the cache key's third component is always equal
  across all such calls, degenerating exactly to today's `(GenericFnId,
  type_args)` behavior. The existing MAP-020/MAP-025 test suites (`identity`,
  `count`, non-callback generic impls) must keep passing byte-for-byte
  unchanged as a regression check.

## Migration Plan

1. Add `Out.callback_bindings: hir::Bindings` and thread a
   `callback_key: &[Option<hir::CallableId>]` parameter through `check_body`
   (Decision 2), with the three existing non-generic call sites passing an
   all-`None`/empty default — covered by the full existing test suite,
   which must keep passing byte-for-byte unchanged.
2. Compute `callback_key` in `generic_call` (Decision 1) and widen
   `Out.instances`/`Out.building` and `instantiate()`'s lookup/build/
   recursion logic to the triple key (Decision 3), keeping the
   polymorphic-recursion scan on `(GenericFnId, type_args)` only.
3. Add focused unit tests: same type args + same callback share one
   instantiation (regression); same type args + different callback produce
   distinct instantiations (new); a callback-forwarding recursive generic
   still terminates at one instantiation; a type-argument-diverging
   recursion is still rejected with the existing "polymorphic recursion"
   diagnostic and message shape, unaffected by an unrelated differing
   callback elsewhere in the same program.
4. Add a CLI diagnostic test confirming the existing polymorphic-recursion
   CLI test (`tests/cli.rs`) is unaffected, plus a new CLI-level snapshot
   confirming two differently-bound calls to the same generic instantiate
   separately (observable via, e.g., a diagnostic or dump path already used
   by existing instantiation-count tests).
