## Context

See `proposal.md` for motivation and
`specs/closure-literal-syntax/spec.md` for the contract. Confirmed
empirically against the current tree (`cargo run -- <file>`):

- `fn(...)` today only parses in two places: as a named top-level
  declaration (`parse_item`'s `Tok::Fn` arm calls `sig()` — which requires an
  identifier right after `fn` — then `block()`), and as a **type annotation**
  (`ty()`'s `Tok::Fn` arm, which requires an explicit `->` and produces
  `ast::TypeKind::Callable { params: Vec<Type>, result: Box<Type> }` from a
  comma-separated list of bare types, no names). Neither path accepts a
  nameless `fn(...)` followed by a body block in expression position — the
  `Tok::Fn` token has no arm in `primary()`, so it hits the wildcard
  `"式が必要です (実際は {:?})"` diagnostic today.
- `ast::Sig` (the named-function signature) and `ast::Param` already carry
  exactly the fields a closure literal needs: `Vec<Param>` (each `{ name,
  ty: Type }`) and `Option<Type>` for the return. `sig()`'s parameter-list
  loop (`src/parse.rs:506-521`) already implements CLO-Q1's grammar
  (mandatory `name: Type`, optional trailing `-> Type`, omitted arrow ⇒
  `unit` per `typecheck::effective_ret`) — a closure literal needs the
  identical loop, just without the leading name/type-params/receiver `sig()`
  also parses.
- `ast::ExprKind` is matched exhaustively (no wildcard arm) in three places
  outside the parser: `typecheck::walk_kind`'s value-position match
  (`src/typecheck.rs`) and two passes in `src/module.rs` (expression
  name/import resolution `resolve_expr`, and local-binding-path collection
  `collect_expr_paths`). Adding a new `ExprKind` variant fails to compile
  until each of these gains an arm — the same mechanical, compiler-guided
  update IDX-010 made for `ExprKind::Index`.
- Unlike `Index`, a closure literal's body is not just more sub-expressions
  in the current scope — its `params` introduce new local names, and per
  CLO-Q2 the body may also reference the *enclosing* function's locals
  (capture). `module.rs`'s two passes already have an established pattern
  for "introduce new locals, recurse with an extended scope, don't leak
  back out" (`Head::For`'s loop variable, `Match`'s per-arm payload
  bindings): clone the current `locals` set, insert the new names, recurse
  with the clone. This is what a closure's params need too, and — unlike
  CLO-020's free-variable *resolution* — doing so here is purely mechanical
  bookkeeping for the module-name-vs-local disambiguation these two passes
  already perform for every other binding form; it makes no claim about
  capture legality, ownership, or which specific outer locals end up
  captured.

## Goals / Non-Goals

**Goals:**
- Parse `fn(params -> ret) { body }` as a primary expression per CLO-Q1's
  grammar, reusing the named-function parameter/return grammar exactly
  (mandatory annotations, no inference), usable anywhere an ordinary
  expression is valid.
- Give the closure literal's declared parameter/return types the identical
  representation (`ast::Type`, `ast::Param`) already used by named function
  signatures and by the `fn(P1, P2 -> R)` type annotation, so CLO-020 needs
  no new type representation to make closures conform where a callable type
  is expected (CLO-Q3).
- Keep every non-closure program's parse output, diagnostics, and (via the
  mechanical exhaustive-match updates) module-resolution/type-check behavior
  byte-identical.
- Reject malformed closure literals with a source-positioned diagnostic,
  consistent with existing brace/paren error reporting.

**Non-Goals:**
- Resolving which enclosing locals a closure body actually references,
  enforcing move/copy capture rules, or checking the body's expressions
  against its declared parameter/return types (CLO-020).
- Ownership of the captured environment, drop timing, or aggregate (struct
  field / array element) storage of closure values (CLO-030, CLO-020's
  aggregate-storage half).
- Specialization keys, ambient-requirement inference, or any interpreter or
  Wasm execution of a closure literal (CLO-040 through CLO-070) — this task
  does not lower `Closure` to `hir` at all.
- Any new syntax or behavior for named `fn` items or the `fn(P1, P2 -> R)`
  type annotation — both already parse today and are unaffected.

## Decisions

### 1. Add one AST variant; factor `sig()`'s param/return loop into a shared helper

`ast::ExprKind` gains:

```rust
Closure {
    params: Vec<Param>,
    ret: Option<Type>,
    body: Vec<Expr>,
},
```

`sig()`'s existing parameter-list-then-optional-return parsing
(`src/parse.rs:506-521`) is extracted into:

```rust
/// `(a: int, b: str -> R)` の中身。名前付き関数の `sig()` と無名関数
/// リテラルで共有する — 引数名・型注釈は必須、期待型からの推論はしない (CLO-Q1)
fn fn_params_and_ret(&mut self) -> PResult<(Vec<Param>, Option<Type>)> { ... }
```

`sig()` calls this helper for its own parameter list (behavior-preserving —
same tokens consumed, same diagnostics), and `primary()` gains:

```rust
Tok::Fn => {
    self.bump();
    self.expect(&Tok::LParen, "`(`")?;
    let (params, ret) = self.fn_params_and_ret()?;
    self.expect(&Tok::RParen, "`)`")?;
    let body = self.block()?;
    ExprKind::Closure { params, ret, body }
}
```

This gives the closure literal exactly the named-function parameter/return
grammar for free, with zero duplicated parsing logic to keep in sync.
`self.block()` is the same body parser named functions and tests already
use, so `{}`, single-expression, and multi-expression bodies all work
unchanged.

Alternative considered: give closures their own bespoke param-parsing
function, independent of `sig()`. Rejected — CLO-Q1 explicitly requires the
same mandatory-annotation grammar; duplicating the loop risks the two
productions drifting (e.g. one accepting an inferred type the other
rejects) with no requirement motivating the duplication.

### 2. `fn` disambiguation needs no lookahead beyond where it already exists

Three positions already dispatch on `Tok::Fn` today, each in a disjoint
grammar context that already determines which production applies before any
new ambiguity could arise: `parse_item` (item position — `fn` must be
followed by a name), `ty()` (type-annotation position — `fn` is always
followed directly by `(` with bare types inside), and now `primary()`
(expression position — `fn` is always followed directly by `(` with
`name: Type` pairs inside). No token of lookahead is added; the parser
already knows which of the three contexts it is in by construction (which
top-level `fn` production or expression-parsing function it called into),
so `fn` followed by `(` in expression position is unambiguously the new
closure literal and cannot be confused with the type annotation (which only
ever appears where a `Type` is expected, e.g. after `:` or `->`, never as a
standalone expression).

### 3. Malformed literals fail through the existing `expect`/`err` machinery

A missing `)` after the parameter list, or a missing `{`/`}` around the
body, is rejected by the same `self.expect(...)` calls other bracketed forms
already use (`block()`'s existing `Tok::LBrace`/loop-until-`Tok::RBrace`
handling, `fn_params_and_ret`'s reuse of `sig()`'s existing
`self.expect(&Tok::RParen, "`)`")?`), producing a source-positioned
diagnostic via the parser's existing error type. A missing parameter type
annotation (`fn(x -> int) { x }`) is rejected by the same
`self.expect(&Tok::Colon, "`:`")?` call `sig()` already uses for named
functions — no new diagnostic message is added for any of these; the
closure literal reuses the exact wording named-function parsing already
produces for the identical omission.

### 4. Downstream exhaustive matches: `module.rs` extends the locals scope, `typecheck.rs` stays inert

`ExprKind::Closure` needs an arm in the three exhaustive matches identified
in Context:

- `src/module.rs` (`resolve_expr`, expression name/import resolution):
  resolve each parameter's type and the return type via the existing
  `resolve_type` (same call `resolve_sig_types` already makes for a named
  function's params/ret), then recurse into `body` with a **cloned and
  extended** `locals` set — `locals.clone()` plus each parameter name
  inserted — mirroring `Head::For`'s `inner_locals` pattern exactly. This is
  the minimum needed so that (a) type names inside parameter/return
  annotations get canonicalized the same way a named function's do, and (b)
  a closure body referencing its own parameters isn't misdiagnosed as an
  unresolved module reference. It says nothing about which *enclosing*
  locals are legally captured — the enclosing `locals` set is still passed
  through unchanged (cloned, not replaced), so references to outer locals
  resolve exactly as they already do inside any other nested scope (e.g. a
  `Head::If` body) — this pass has never enforced capture legality for any
  construct and does not start doing so here.
- `src/module.rs` (`collect_expr_paths`, local-binding-path collection):
  same shape, mirroring the existing `Head::For` arm.
- `src/typecheck.rs` (`walk_kind`'s value-position match): reports a single
  "この版では未対応です"-style diagnostic and poisons immediately —
  `ExprKind::Closure { .. } => { out.push(...); poison() }` — **without**
  descending into `params`, `ret`, or `body`. This is the one deliberate
  asymmetry versus IDX-010's `Index` stub (which did `synth()` both
  sub-expressions to keep reporting nested errors): `Index`'s sub-
  expressions are ordinary already-scoped expressions with no new bindings,
  so synthesizing them costs nothing extra. A closure's `body` is only
  meaningful under a scope extended with its own parameters and (per
  CLO-Q2) capture of specific enclosing locals — walking into it now would
  mean either inventing a throwaway scoping/capture rule here (duplicating
  work CLO-020 must define properly) or silently treating captures as
  ordinary local lookups (which would accept programs before CLO-020 has
  decided their capture legality). Not descending keeps this stub strictly
  representational: it establishes that the AST shape exists and that its
  declared type maps to the same `Callable` representation (Decision 5
  below, proven at the parser level, not by exercising this stub), while
  leaving 100% of body semantics for CLO-020 to add to this single arm.

Because the closure literal is never usable as a value (it always poisons),
no other typecheck.rs code path (call-argument matching, `let` initializer
matching, array/struct element checks) needs a `Closure` case — those paths
all go through `synth`/`walk`, which already treat a poisoned sub-expression
uniformly regardless of which `ExprKind` produced the poison.

Alternative considered: have the stub compute a `KnownType::Callable` from
the closure's own annotations and return `Outcome::Typed` instead of
`Outcome::Poisoned`, so a closure literal could already flow into a
matching `let f: fn(int -> int) = ...` slot. Rejected — `Outcome::Typed`
with no corresponding `hir::ExprKind` other than `Poison` would require
inventing a `hir::ExprKind::Closure` (and downstream HIR/ownership/eval/wasm
arms for it) purely to hold a value that can never actually be evaluated
yet, which is exactly the CLO-060/070 work this task explicitly excludes;
`poison()` already exists for "the type shape is known in principle but
this expression cannot be used yet."

### 5. The "same type as named function values" condition is proven at the parser level, not by exercising the typecheck stub

Since `walk_kind`'s stub never computes a `KnownType` for a closure literal
(Decision 4), CLO-010's completion condition "closure 値は既存の named 関数値
と同じ型 `fn(P1, P2 -> R)` として型検査へ渡る" is satisfied **structurally**
by construction — `ExprKind::Closure.params: Vec<Param>` and `.ret:
Option<Type>` are the identical field types `ast::Sig` already uses, so
there is no separate "closure type" to keep in sync — and is **locked in by
a focused parser test** that parses both a named function's
`fn(P1, P2 -> R)` type annotation and an equivalent closure literal's
`params`/`ret` fields, converts both through the same
`ast::TypeKind::Callable` construction, and asserts the two are equal. This
is the same "prove it at the level where the fact is true" approach IDX-010
used for `len()` (a locking regression test with no new parsing code, since
`len()` already worked) — here the fact being locked in is a type-shape
equality rather than a parse-shape equality.

## Risks / Trade-offs

- [Adding `ExprKind::Closure` forces edits in `module.rs` (x2) and
  `typecheck.rs` (x1) even though this task is scoped to grammar/type
  representation] → Unavoidable given exhaustive matching (Decision 4);
  kept to the minimum shape (mechanical scope extension in `module.rs`,
  zero-semantics stub in `typecheck.rs`) so CLO-020 replaces exactly one
  match arm's body, not a new pass.
- [Not descending into the closure body in the `typecheck.rs` stub
  (Decision 4) means a syntax error or unresolved name *inside* a closure
  body is reported only as the one blanket "unsupported" diagnostic on the
  closure itself, not pinpointed to the inner problem, until CLO-020 lands]
  → Accepted as temporary and consistent with this task's own non-goals:
  CLO-010's verification scope (ROADMAP's 検証方法) is parser-level only, and
  a poisoned closure literal already halts compilation before any use site
  could depend on a more precise inner diagnostic.
- [`module.rs`'s `resolve_expr`/`collect_expr_paths` arms extend the locals
  scope for the closure body (Decision 4), which is slightly more than the
  "purely mechanical, no-semantics" plumbing IDX-010's `Index` stub needed]
  → Necessary, not optional: without it, every closure parameter reference
  inside the body would be misdiagnosed as an unresolved module path by an
  existing, unrelated diagnostic, which would make even the "does this
  parse and reach type-checking" scenario in the spec fail before reaching
  `typecheck.rs` at all. The extension only ever adds names to a scope this
  pass already threads through nested constructs; it decides nothing about
  capture.

## Migration Plan

1. Add `ExprKind::Closure`, the shared `fn_params_and_ret` helper, and the
   `primary()` case, with `docs/grammar.md` updated in the same commit.
2. Add the mechanical `module.rs` (x2) and `typecheck.rs` (x1) match arms
   needed to keep the build green, per Decision 4.
3. Add focused parser tests (zero/multi-parameter closures, omitted return,
   malformed literals, non-interference with existing `fn` items/type
   annotations/blocks, the type-shape-equality lock from Decision 5) and a
   `tests/cli.rs` diagnostic test for a malformed closure literal; run the
   full suite, `cargo fmt --check`, and warnings-as-errors Clippy.
4. Commit the verified, test-passing state as a single stable snapshot.
5. Rollback is a plain revert: no runtime path, persisted data, or public
   ABI is touched, and no existing fixture uses a closure literal.
