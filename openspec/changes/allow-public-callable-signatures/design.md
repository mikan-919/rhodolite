## Context

See `proposal.md` - Why. This section only records the pipeline facts the
approach depends on.

- `module::load` (`src/module.rs`) resolves `pub use` re-exports and returns
  `LoadedProgram::public_exports: Vec<PublicExport>` (`name`, `canonical`
  fully-qualified declaration name, `span`) **before** `typecheck.rs` runs.
  Both CLI paths in `src/main.rs` (plain interpret and `build`) call
  `typecheck::check_and_lower(&program)` on the same `loaded.program`, so
  whatever `check_and_lower` accepts or rejects is identical regardless of
  which command runs.
- `src/typecheck.rs::lower_ret` unconditionally calls `reject_callable` on
  every function's return type (line ~1327), independent of visibility. This
  is the existing "in this change" restriction from `named-function-values`
  (`design.md` decision 2 of that change). Callable-typed *parameters* are
  already permitted for every top-level function via
  `check_signature_shape`'s `callback_ok = true` path
  (`reject_nested_callable`); that path already enforces "outermost only, no
  optional, no reference, no nesting" - CAB-010 return support must apply the
  same shape rule, not the coarser `reject_callable`/`has_callable`.
- `hir::callable_of(body, expr_id, bindings: &Bindings)` (`src/hir.rs`)
  already resolves an expression to the one `CallableId` it statically names,
  given a `Bindings = BTreeMap<LocalId, CallableId>` map. Production roots
  (`main` and each public export) are planned from an **empty** `Bindings`
  in `ambient_abi::plan_roots` (`Bindings::new()`), because nothing outside
  the wasm program supplies their callback arguments. The same empty
  `Bindings` is the right context to resolve "what does this public export's
  callable-typed return path name."
- `requirement::Analysis::requirements(body: hir::BodyId, bindings: &Bindings)
  -> &BodyReqs` (`src/requirement.rs`) already returns the fully-inferred
  ambient requirement set for one specialization. `BodyReqs` empty means zero
  ambient requirement. This is computed once by `requirement::analyze` in
  both CLI paths, so looking up a target function's own (unbound) requirement
  set needs no new inference machinery.
- `src/main.rs::build()` already runs a public-export-aware check that the
  interpret path does not: `analysis.errors_for_roots(&roots)`, covering
  `main` plus every public export. The ambient-zero check this change adds is
  the same kind of check (public-export-scoped, needs nothing beyond
  `checked.hir` and `analysis`) and belongs at the same call site, not inside
  `wasm_abi.rs` or `wasm.rs`.
- `wasm::supported(ty)` (`src/wasm.rs`) already returns `true` for
  `TypeKind::Callable { .. }`, because that predicate answers "can the
  backend represent this type **internally**" (true - callable locals/params
  are resolved to direct calls at compile time, never given a runtime
  representation). `wasm_abi.rs::param_port`/`result_port` reuse the same
  predicate to answer a different question - "can this type cross the ABI
  wire" - and currently fall through to `Port::Rich(ty.clone())` for any
  `supported` non-scalar type. Nothing in `wasm_data.rs`/`Schema::describe`
  knows how to encode a callable; `Schema::describe`'s match has no
  `Callable` arm and ends in `unreachable!("表に載らない型です: {other:?}")`.
  Before this change nothing could reach that arm, because typecheck always
  rejected a callable return and `wasm_abi.rs` was the only place that ever
  ran `param_port` on a public parameter's declared type. Once this change
  makes a callable-typed public signature legal at typecheck, that
  `unreachable!` becomes reachable from valid source unless `wasm_abi.rs` is
  updated in the same change.

## Goals / Non-Goals

**Goals:**
- Let a public export declare a callable return type, mirroring the existing
  parameter allowance, gated on the function being an actual public export
  (not a general lift of the `named-function-values` restriction).
- Enforce CAB-Q3 (named-function only) and CAB-Q4 (ambient-zero) for every
  callable-typed public export's return value, with a source span, before
  Wasm generation.
- Keep `rhodolite build` free of panics for the newly-legal signatures by
  making the Wasm backend explicitly reject what it cannot yet generate.

**Non-Goals:**
- Any actual handle representation, invoke export, or ABI v1 metadata shape
  for callables - CAB-020/030/040.
- Checking callable-typed public *parameters* for ambient-zero by resolving
  a concrete target function. There is no static target: CAB-Q1 restricts
  the direction to wasm-authored callables, so any value reaching such a
  parameter is a handle round-tripped through the host from some other
  public export's return, and that origin is already checked at its own
  return site. See Decision 4.
- Lifting the `named-function-values` return-value restriction for
  non-public functions. Only public exports gain the new capability.
- Closures. `ExprKind::Closure` is rejected everywhere today (CLO-020 has
  not landed); this change adds no closure-specific code, only a regression
  test.

## Decisions

### 1. Thread `public_exports` into `typecheck::check_and_lower`, at both CLI call sites
`check_and_lower(program: &Program, public_exports: &[module::PublicExport])`.
Build a `BTreeSet<&str>` of `canonical` names once, before the per-item
loop, and consult it by `sig.name` (already fully-qualified by `module.rs`)
wherever `lower_ret` needs to decide whether a callable return type is this
function's business.

Alternative considered: scope the relaxation to `build()` only, since
`public_exports` only has teeth there. Rejected because `check_and_lower` is
one function shared by both CLI paths; making its accept/reject decision
depend on which command the caller happens to be running would mean the same
source file type-checks differently under `rhodolite file.rd` and
`rhodolite build file.rd --target wasm`. Passing `public_exports` at both
call sites keeps typechecking command-independent, at the cost of the
interpret path threading a value it never otherwise uses.

### 2. Return-type relaxation reuses the parameter's shape rule, gated by public-export membership
Replace `lower_ret`'s unconditional `reject_callable(ty, ...)` with logic
that, when `sig.name` is in the public-export set, calls the equivalent of
`reject_nested_callable` (outermost callable only, no optional/reference/
nested aggregate) instead of blanket-rejecting; otherwise keeps calling
`reject_callable` exactly as today. Non-public functions see no behavior
change.

### 3. New ambient-zero + resolved-target check runs in `build()`, next to `errors_for_roots`
Add a new check (e.g. a function in `requirement.rs`, since it only needs
`checked.hir` and `analysis`, or a small new module if `requirement.rs`
should stay backend-agnostic of "public export" as a concept - implementer's
call) that, for each `(name, CallableId)` in the `exports` list `build()`
already builds:
- Skips it unless `callable.ret` is a `Callable` type.
- Walks every value-producing return path in the callable's body (explicit
  `return e` and the final expression), resolving each with
  `hir::callable_of(&callable.body, expr_id, &hir::Bindings::new())`.
- `None` → diagnostic at that expression: the return path does not name one
  static function.
- `Some(target)` → look up
  `analysis.requirements(hir::BodyId::Callable(target), &hir::Bindings::new())`;
  non-empty → diagnostic at that expression naming `target`'s declared name
  and the required slot name(s), matching the existing
  "missing provider" message shape used by `errors_for_roots`/
  `unsatisfied_for` for consistency.

This runs after `requirement::analyze` and before `ambient_abi::plan_production`,
alongside the existing `errors_for_roots` call, so both classes of
public-boundary diagnostics surface together before any Wasm-specific work
starts.

Alternative considered: put this check inside `wasm_abi.rs::signatures_impl`,
which already has exactly the right scope (`production.exports`) and already
produces "can't cross the boundary" diagnostics. Rejected: `signatures_impl`
only runs from `wasm::emit`, deep inside `build()`, after
`ambient_abi::plan_production` has already succeeded - by design it checks
*shape*, not ambient requirements, and duplicating `Analysis`/`Bindings`
plumbing there would fight the module's existing "only checks what's
selected as public, only checks representability" scope. Keeping the check
next to `errors_for_roots` also matches how CAB-Q4's `named-function-values`
precedent already reports missing-provider errors: as a `requirement`-driven
check before planning, not as a Wasm-shape check.

### 4. No new check for callable-typed public parameters
A callable value can only be *created* as a direct reference to a named
top-level function (CAB-Q3; closures remain unsupported). Per CAB-Q1, host
never authors its own callable - the only way host can hand wasm a callable
handle back through a public parameter is by round-tripping a handle wasm
itself returned earlier. That origin already went through Decision 3's
check at its own return site. So a callable-typed public *parameter* needs
no additional static analysis here: if the receiving function tries to
*call* that parameter internally (`f()`), `ambient_abi::plan_roots` already
plans every root (including public exports) from an empty `Bindings`, and an
unbound indirect call there already produces `PlanError::UnresolvedCallback`
- pre-existing behavior this change does not need to duplicate. If the
function merely stores or forwards the parameter (the only other legal
position for a callable value), there is nothing to check.

### 5. `wasm_abi.rs` gets an explicit `Callable` case, not a reused `supported()` branch
In `param_port`/`result_port`, add a case for
`matches!(ty.kind, hir::TypeKind::Callable { .. })` before the
`None if supported(ty) => Port::Rich(...)` fallback, producing a
source-spanned "Wasm ターゲットではまだ生成できません" diagnostic (naming
CAB-030 in the help text) instead of routing into `Port::Rich`. This keeps
`wasm::supported()` unchanged (it is still correct for the *internal*
reachability question `check_support`/`check_local` ask) and keeps
`Schema::describe`'s `unreachable!()` genuinely unreachable, since a
callable-typed public signature now fails in `param_port`/`result_port`
before metadata generation ever sees it.

Alternative considered: change `wasm::supported()` to exclude `Callable`
entirely. Rejected: `check_support`/`check_local` (internal reachability)
correctly treat callable as representable today, since internal callable
locals/params never need a wire form - only the public-boundary functions in
`wasm_abi.rs` need the stricter answer, and they already have their own
dedicated checking functions to put it in.

## Risks / Trade-offs

- [Threading `public_exports` through `check_and_lower` touches its public
  signature and both call sites] → small, mechanical change; both call
  sites already have `loaded.public_exports` in scope.
- [The new ambient-zero check and the existing `errors_for_roots` check can
  both fire for unrelated reasons on the same build] → both already collect
  into `Vec<Diag>` and get reported together by `render::report`; no new
  fail-fast behavior needed, matches existing multi-diagnostic reporting.
- [A future change (CAB-030) will delete the `wasm_abi.rs` "not yet
  supported" rejection and replace it with real codegen] → expected and
  intentional; the delta spec added to `rhodolite-wasm-abi` here should be
  superseded by CAB-030's own delta rather than staying "ADDED" forever -
  noted for CAB-030's proposal, not actionable now.

## Open Questions

None - CAB-000 already settled the cross-cutting questions (CAB-Q1〜CAB-Q5);
what remained here was where in this codebase's existing pipeline to place
the checks, which the Decisions above resolve.
