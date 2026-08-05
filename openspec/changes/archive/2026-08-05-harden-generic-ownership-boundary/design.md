## Context

See `proposal.md` for motivation and `specs/generic-ownership-boundary/spec.md`
for the contract. This picks up exactly where
`openspec/changes/archive/2026-08-05-add-generic-function-instantiation/`
(MAP-020) and `openspec/changes/archive/2026-08-05-add-generic-trait-resolution/`
(MAP-025) left off.

MAP-010 (`archive/2026-08-05-add-generic-type-parameters/`) drew a hard line
between two type representations in `src/hir.rs`: `hir::Type`/`TypeKind` (no
`Param` variant — used by every concrete `hir::Callable`, `hir::Body`, and
everything ownership checking, requirement analysis, the interpreter, and
Wasm generation read) and `hir::GenericType`/`GenericTypeKind` (adds exactly
one leaf, `Param(TypeParamId)` — used only by `hir::GenericDecl`, which lives
in `Program.generics` and is never registered in `Ids::fns`/`trait_methods`/
`methods`/`trait_impls`). `hir::substitute(generic_ty, subst) -> Type`
(`src/hir.rs:322`) is the only place a `GenericType` becomes a `Type`; every
leaf resolves to a genuine concrete `TypeKind` variant except an uncovered
`Param`, which becomes `TypeKind::Poison` — and `typecheck::check_and_lower`
already rejects any `hir::Program` containing `Poison` before returning
`Ok` (`out.lowered.poisoned()`, `src/typecheck.rs:640`), so `ownership::check`
(`src/ownership.rs:422`) is only ever called with an `hir::Program` that
already cannot contain an unresolved type parameter or a poisoned leaf. This
is a structural guarantee (a value of the disallowed shape cannot be
constructed as a `Type`), not a runtime check — MAP-010's own comment on
`GenericType` says as much: "`TypeKind` 側には `Param` を足さない。だから
ownership・要求解析・interpreter・Wasm 生成が読む具体 HIR に型変数が入る形は
**構造として存在しない**".

`typecheck::instantiate` (`src/typecheck.rs:2259`) is the one function that
produces a concrete `hir::Callable` for both a generic free function
(MAP-020) and a generic impl method (MAP-025, via `instance_owner`,
`src/typecheck.rs:2366`) — both paths share the same cache
(`out.instances: Vec<(GenericFnId, Vec<hir::Type>, CallableId)>`), the same
"currently instantiating" stack, and the same `hir::substitute` call for the
return type before the body is even checked. `ownership::check` iterates
`hir.bodies` (`src/ownership.rs:426`) generically; nothing in
`src/ownership.rs` matches on `hir::CallableOwner` or on whether a
`CallableId` came from `collect()`'s declaration pass or from `instantiate`.
Both MAP-020's and MAP-025's own design docs state this was already true by
construction and explicitly defer the dedicated ownership-behavior test
matrix to this task (MAP-020 tasks.md 5.3 added exactly one such test, for
`apply`'s consuming callback with a free function; MAP-025's spec Non-Goals
names "any new ownership-specific rule or dedicated ownership test matrix
(MAP-030)" verbatim).

Concretely, before this change, `src/ownership.rs`'s test module
(`src/ownership.rs:2733`) has exactly one generic-aware test
(`汎用関数越しの非copy引数はちょうど1度moveされる`, covering a free-function
consuming callback) and none for: Copy classification differing across two
instantiations of the same declaration, move-after-use on a non-`Copy`
instantiation, drop planning inside an instantiation, or any generic impl
method shape.

## Goals / Non-Goals

**Goals:**
- Give the "ownership only ever sees concrete HIR" guarantee an explicit,
  observable spec (`generic-ownership-boundary`) instead of leaving it
  implicit in MAP-010/MAP-020/MAP-025's design notes.
- Add the ownership-behavior test matrix both prior tasks deferred: Copy vs.
  non-Copy classification across distinct instantiations, move-after-use
  rejection, deterministic drop planning, and a consuming callback moving a
  non-`Copy` value exactly once — each for both a generic free function and
  a generic impl method.
- Add a regression test that makes "no type variable reaches ownership
  checking" fail loudly if a future change ever breaks it, rather than
  relying solely on the type system continuing to make the bad shape
  unconstructable.
- Fix any gap the new tests uncover in `src/ownership.rs`/`src/typecheck.rs`
  as part of this change.

**Non-Goals (per ROADMAP.md's task boundary):**
- Widening the whole-program specialization key with callback identity or
  trait-impl choice — MAP-040.
- Ambient/effect requirement inference through a callback-typed parameter —
  MAP-050.
- Running a generic instantiation in the HIR interpreter or generating it to
  Core Wasm — MAP-060/MAP-070. This task's tests exercise ownership
  checking's plan (moves/borrows/clones/drops), not execution.
- Any new language surface, public API, data representation, or runtime
  contract. `hir::Type`/`GenericType` already have the shape this task
  relies on; no change to either is expected.

## Decisions

### 1. Treat the boundary as already implemented; scope the change to verification, with a fix-on-discovery fallback

Because `TypeKind` has no `Param` variant, an instantiation with a leftover
type variable cannot be represented, let alone reach ownership checking —
there is no runtime "detect and reject" step to add for the literal reading
of "型変数が ownership 以降へ漏れた場合は internal invariant violation とし
て検出する". The closest thing to a runtime detection point already exists:
`hir::substitute`'s uncovered-`Param` case producing `TypeKind::Poison`,
caught by `poisoned()` before `ownership::check` ever runs (see Context).
This task does not duplicate that check inside `ownership.rs` — doing so
would require `Callable`/`Body` to carry a "might be unresolved" tag that
the type system already makes impossible, i.e. reintroducing by hand what
MAP-010 deliberately removed by construction.

Instead, this task's "invariant test" is the regression test in Decision 3:
an observable proxy for "a type variable leaked" that would actually fail if
someone regressed the MAP-010 split (e.g. by giving `TypeKind` a `Param`
variant and wiring a partial substitution through it) or the MAP-020/MAP-025
substitution call sites (e.g. by skipping `hir::substitute` and copying a
`GenericType`-shaped value into a `Callable` field typed `Type` — which
would not compile, but the rigid-check placeholder test also catches the
adjacent mistake of leaking the *rigid* body's synthetic names into a real
instantiation).

**Alternative considered:** add a `debug_assert!`/invariant-walk function in
`ownership::check` that recursively inspects every `Type` reachable from
`hir.callables` for some marker of "came from an unresolved substitution".
Rejected — there is no such marker to look for (a `Poison` here already
means "diagnosed and rejected earlier," not "type variable," and conflating
the two would make the assertion either always-true-and-useless or subtly
wrong); the type system already provides the guarantee an assertion would
be trying to approximate at runtime, at zero cost and with no possibility of
the assertion itself going stale.

### 2. The test matrix reuses `src/ownership.rs`'s existing test harness and dump format, doubled across free function and impl method shapes

Every new test is a `#[cfg(test)]` case added to `src/ownership.rs`'s
existing `mod tests` (`accepted`/`rejected`/`only`/`dump`/`effects`, already
used by the one existing generic test). No new test infrastructure is
introduced. Each of the four completion-condition behaviors (Copy vs.
non-Copy classification, move-after-use, drop, consuming callback) gets one
test using a generic free function (extending MAP-020's existing pattern)
and one using a generic impl method (the newly-reachable MAP-025 shape its
own spec deferred), except where a free-function test already exists
(`apply`'s consuming callback) — that one is kept as-is and only the impl
method counterpart is added, to avoid duplicating an already-passing
scenario.

The "rigid-check placeholder never leaks" regression test asserts the dumped
ownership plan (`checked.plan.dump(&checked.hir)`) of a program with at
least two distinct instantiations of the same generic declaration (using
differently-named type parameters, e.g. both `T` and `U`) does not contain a
`#`-prefixed name (`rigid_name`, `src/typecheck.rs:1876`, always formats a
rigid-check placeholder as `#<param-name><index>`, e.g. `#T0`/`#U1` — the
lexer never produces `#` inside an identifier, confirmed by
`src/lex.rs:456`'s existing regression test, so `#` cannot appear in any
real declared or synthesized name). This character can only appear in a
dump if a rigid (never-lowered, `Target::Discard`) body's synthetic name
somehow ended up in a real, kept `hir::Body`, which is exactly the failure
mode "型変数が漏れた" describes in observable terms.

**Alternative considered:** write the invariant test as a standalone
property test that constructs arbitrary `GenericDecl`s and fuzzes
`substitute`. Rejected — `substitute`'s two cases (covered `Param` → the
bound `Type`; uncovered `Param` → `Poison`) are already exhaustively unit
tested (`substituteは型パラメータを型引数へ置き換える`, `src/hir.rs:1804`);
a property test here would exercise the same two branches with more
machinery, not a different one. The end-to-end dump-based regression test
instead covers the thing a unit test on `substitute` alone cannot: that
*nothing between* `substitute` and the checked ownership plan reintroduces a
placeholder.

### 3. No production code change is assumed; any gap found is fixed in this change, not deferred

Per Decision 1, the design's expectation is that every new test in the
matrix already passes against the current `src/ownership.rs`/
`src/typecheck.rs`, since MAP-020/MAP-025 already route every instantiation
through the ordinary pipeline. If a test fails, the fix belongs to whichever
of `src/ownership.rs` (a genuinely missing ownership rule) or
`src/typecheck.rs` (a genuinely incomplete substitution/instantiation path)
the failure points to — this task does not introduce a workaround or narrow
the test to avoid the case, since "具体型の Copy / owned 分類で move、
borrow、clone、drop を計画する" is a completion condition, not a nice-to-have.

## Risks / Trade-offs

- [The regression test's `#`-prefix check is a textual proxy, not a
  structural one] → Accepted: a structural check is unnecessary because the
  type system already forbids the bad shape (Decision 1); the textual check
  exists to catch a *regression* in that guarantee, and `#` is specifically
  chosen because the lexer already refuses it inside any identifier
  (`src/lex.rs:456`), so a false positive would require a lexer change that
  starts accepting `#` in identifiers — itself a change worth flagging.
- [Doubling the test matrix across free-function and impl-method shapes
  increases `src/ownership.rs`'s test count] → Accepted: this is the
  explicit scope MAP-025's own spec deferred here; the tests are small,
  mechanical variations of the existing generic test pattern, not new
  infrastructure.

## Migration Plan

1. Add the Copy/non-Copy classification test pair (free function, impl
   method), confirming the same declaration is classified differently per
   instantiation.
2. Add the move-after-use rejection test pair.
3. Add the drop-planning test pair.
4. Add the impl-method counterpart of the existing consuming-callback test
   (the free-function version already exists from MAP-020).
5. Add the rigid-check-placeholder regression test.
6. If any test in 1-5 fails, fix the gap in `src/ownership.rs`/
   `src/typecheck.rs` before proceeding, then re-run the full suite.
7. Run `cargo fmt --check` and Clippy with warnings as errors; commit the
   stable, passing state as a single snapshot.
8. Rollback is a plain revert: this change adds tests (and, only if
   necessary, a small ownership/typecheck fix) with no public API, data
   representation, or runtime contract change.
