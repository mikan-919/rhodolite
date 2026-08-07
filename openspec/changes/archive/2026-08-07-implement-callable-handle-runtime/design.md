## Context

See `proposal.md` - Why. This section only records the interpreter facts the
approach depends on.

- `src/eval.rs` already has two layers: `CheckedInterp<'p>` (a per-call frame:
  fresh `Store`/heap, created by `CheckedInterp::new` and disposed at the end
  of `run`/`run_test`) and the thin public wrapper `Interp<'p>` (`program`,
  `plan`; `run`/`run_test`/`show` all take `&self`). A callable value's
  runtime representation, `OwnedValue::Function(hir::CallableId)`, is just an
  arena index into `hir::Program::callables` — a named-function value is
  `Copy`, captures nothing, and needs no store slot (design.md decision 9 of
  `named-function-values`, still in force). This means a handle standing for
  one callable value needs no reference to any particular call's `Store`; it
  only needs the target `CallableId`, which stays valid for the whole
  program's lifetime.
- `CheckedInterp::public_value`/`checked_public_value` is the one place that
  turns an internal `OwnedValue`/`CheckedValue` into the externally
  observable `eval::Value` returned by `run`/`run_test`. Today its
  `OwnedValue::Function(_)` arm unconditionally fails
  (`"callable 値は観測できる値になりません"`), a rule written before CAB-010
  legalized a callable-typed public return value.
- `Interp::run(entry: &str)` already resolves *any* free top-level function
  by name via `hir::Program::free_callable` — it is not hardwired to `main`.
  `main.rs` only ever calls it with `main`, but nothing in `run` itself
  assumes that; calling it with a public export's canonical name already
  works today for non-callable-returning exports, and CAB-020 does not need
  to add a new "run an export" entry point.
- CAB-010's own design.md (Decision 4) already establishes that a
  callable-typed public *parameter* needs no new runtime handling here: the
  only way a caller can supply one is by round-tripping a handle this
  change's own invoke operation produced, and using it inside the callee is
  an ordinary indirect call already covered by existing checks. So the one
  conversion point above (`public_value`) is the only place this change
  needs to touch to make a callable value observable at all.
- `differential.rs::classify_interpreter` already pattern-matches
  `Flow::Error` messages into named failure classes for the interpreter side
  of differential testing (`DivisionByZero`, `IntegerOverflow`,
  `EngineTrap`); `Flow::Error(Diag)` is the interpreter's one and only
  existing runtime-failure classification, used for every other runtime
  error (assert failure, division by zero, missing field, ...).

## Goals / Non-Goals

**Goals:**
- Give a callable value that legally crosses the public boundary (per
  `callable-public-boundary`) an actual runtime representation: an opaque,
  single-shot handle, minted at the one point such a value already becomes
  observable.
- Add the runtime operation that consumes a handle to invoke its target,
  and make a second invoke of the same handle fail safely.
- Do this entirely inside the interpreter's existing `run`/`show`-style
  public surface, adding capability without changing the signature of
  anything that already exists.

**Non-Goals:**
- Any Wasm-side representation, invoke export, or ABI v1 metadata for
  callables — CAB-030/CAB-040. This design changes nothing under
  `src/wasm*.rs`.
- Differential fixtures comparing interpreter and Wasm invoke behavior —
  CAB-050. There is no Wasm invoke export yet to compare against.
- General marshalling of struct/enum/array values into or out of the
  invoke operation. See Decision 6.
- A CLI surface for invoking an export interactively. Nothing in
  ROADMAP's CAB-020 entry asks for one; the only consumers of this
  mechanism until CAB-030 are this change's own unit tests.

## Decisions

### 1. The handle table lives on `Interp`, not on `CheckedInterp`, using interior mutability
A handle minted while running one export (a `run` call) must still be valid
for a later, separate invoke call on the same `Interp` value. `CheckedInterp`
is the wrong home: it is created fresh and disposed at the end of every
`run`/`run_test` call. `Interp` is the right home, but its `run`/`run_test`/
`show` methods all take `&self` today, and every existing call site
(`main.rs`, every test helper in `eval.rs`) relies on that. Add the handle
table to `Interp` behind interior mutability (e.g. a `RefCell`-wrapped
table), so this change adds a new method and new internal state without
changing any existing method's signature.

Alternative considered: change `run`/`run_test`/the new invoke method to take
`&mut self` and thread the table explicitly. Rejected: touches every
existing call site for a capability that is purely additive; interior
mutability keeps the blast radius to "one new field, one new method."

### 2. The handle is a fresh opaque identifier minted by the table, not the `CallableId` itself
Even though `CallableId` is already an internal arena index rather than a raw
pointer, using it directly as the "handle" would make the handle a stable,
repeatable value that the language itself can already re-derive (e.g. by
naming the same function again) — nothing about it is single-use unless
every call site remembers to separately consult a used-set keyed by that same
value, which is much easier to get wrong than making the value itself stop
resolving once consumed. Mint a small opaque identifier per handle instead;
the table maps it to the target `CallableId` and to nothing else (Decision 1
already established a handle needs no store-backed data). This is the direct
implementation of ROADMAP's "内部アドレスを含まない不透明 ID" criterion,
where reusing `CallableId` would only be opaque by convention.

Alternative considered: wrap `CallableId` in a newtype and call that the
handle. Rejected for the reason above — it does not by itself give
single-shot semantics, and the ROADMAP wording asks for an ID that is not
derived from an internal address at all.

### 3. A handle's table entry is removed the moment invoke validates it, before the target runs
Remove the handle from the table as soon as invoke looks it up and confirms
it is live — not after the target function returns successfully. This is
the only timing under which "invoking a handle a second time" is rejected
uniformly, regardless of whether the first invoke's target succeeded, failed
with a runtime error, or does anything else. It matches CAB-Q2's phrasing
directly: the handle is released *inside the invoke call*, a property of
making the call, not of the call succeeding.

Alternative considered: remove the entry only after a successful call,
letting a failed invoke be retried with the same handle. Rejected: this
reintroduces exactly the reusable callback CAB-Q2 rules out. CAB-Q2's own
text gives the intended recovery path instead — a host that wants another
attempt calls the exporting function again for a fresh handle.

### 4. Rejecting a second/unknown invoke reuses `Flow::Error(Diag)`, not a new failure category
An invoke of a handle the table does not currently hold (already consumed,
or never minted) returns `Err(Flow::Error(Diag::msg(...)))` — the same
runtime-failure shape every other interpreter runtime error already uses.
ROADMAP explicitly leaves the choice between "trap" and "diagnostic" open
("実装時に既存の failure 分類へ合わせて選ぶ"); `Flow`/`Diag` is the
interpreter's one and only existing classification, and
`differential.rs::classify_interpreter` already knows how to fold `Flow::Error`
messages into named failure classes, so this needs no new machinery on
either side.

Alternative considered: `panic!`. Rejected outright — every other runtime
failure in `eval.rs` (division by zero, assert failure, missing field, ...)
is a `Flow::Error`, not a panic; a double-invoke should not be the one
exception.

### 5. Fix `public_value`'s existing rejection in place; no parallel conversion path
`OwnedValue::Function`'s only current handling is the one rejection arm in
`public_value`. CAB-010's own design.md (Decision 4) already established
that a callable-typed public *parameter* is only ever a round-tripped handle
consumed via an ordinary indirect call, never via `public_value` — so this
one conversion point is the only place a callable value needs to become an
opaque handle. Replace the rejection with handle minting there; add no
second path.

### 6. Invoke's argument/result handling only needs to cover what the required tests exercise
Convert the scalar/unit `Value` cases (`Int`, `Str`, `Bool`, `Unit`, `Nil`)
between the public and internal representations for invoke's arguments and
result — enough to satisfy ROADMAP's two required unit tests (mint + invoke
+ auto-release; double-invoke rejection). A target function whose
parameters or result need struct/enum/array marshalling is not required to
be invokable by this change: report a clear runtime diagnostic rather than
panicking, the same posture CAB-010 already used in `wasm_abi.rs` for Wasm
codegen it deliberately left to CAB-030. Widening this is CAB-030/040/050's
concern once there is an actual host boundary (Wasm ABI wire format) driving
what "argument marshalling" needs to mean.

## Risks / Trade-offs

- [Interior mutability inside `Interp` is new for this struct] → scoped to
  exactly the new handle-table field; `program`/`plan` stay plain
  references, and every existing method's borrowing behavior is unchanged.
- [Decision 3 makes a handle permanently unusable after any failed invoke,
  not just after a successful one] → intentional (CAB-Q2); the documented
  recovery path is calling the exporting function again for a new handle,
  not retrying the old one.
- [Decision 6 is a real scope limit, not a phased rollout inside this
  change] → acceptable because ROADMAP's CAB-020 verification criteria only
  require handle mint/invoke/release and double-invoke-safety tests, not
  general data marshalling. Flagged explicitly so CAB-030/040/050 do not
  assume this change already solved argument encoding across the eventual
  host boundary.

## Open Questions

None. CAB-000 already settled the cross-cutting questions (CAB-Q1〜CAB-Q5);
this design only picks one interpreter-internal shape for CAB-Q2's handle
contract, the same posture CAB-010's own design.md took for its own
questions. Exact Rust-level naming (the handle type's name, the table's
type, which module owns them if not `eval.rs` itself, the invoke method's
name) is intentionally left open here for `tasks.md` / the implementer to
settle, since none of it is externally observable behavior.
