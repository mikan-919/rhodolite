## Why

CLO-000 fixed the observable contract for captured anonymous functions
(ROADMAP.md Decisions CLO-Q1〜CLO-Q5): the literal is `fn(params -> ret) {
body }`, parameter/return annotations stay mandatory, and a closure's static
type is the same `fn(P1, P2 -> R)` shape `named-function-values` already
defines for named top-level functions. Before CLO-020 (capture resolution,
body type-checking, aggregate storage) can be built, the parser needs to
accept this syntax at all. Today `fn(...)` only parses as a **type**
annotation (`ty()`'s `TypeKind::Callable` production); an anonymous `fn(...)
{ ... }` in expression position does not parse — CONTEXT.md's "第二級ブロック"
entry describes this form as the intended first-class counterpart to the
second-class bare `{ ... }` block, but it has never been implemented.

## What Changes

- Add `ExprKind::Closure { params: Vec<Param>, ret: Option<Type>, body:
  Vec<Expr> }` to `src/ast.rs` and parse `fn(params -> ret) { body }` in
  `primary()` (`src/parse.rs`), reusing the exact same mandatory
  `name: Type` parameter grammar and optional `-> Type` (omitted = `unit`,
  never inferred) that `sig()` already uses for named functions, factored
  into a small shared helper so both productions parse identically.
- No lexer change: `Tok::Fn`, `Tok::LParen`, `Tok::Arrow`, and `Tok::LBrace`
  already exist and are already used together for named function items;
  only a new `Tok::Fn` arm in `primary()`'s expression-start match is added.
- Add the minimal exhaustive-match plumbing this new `ExprKind` variant
  forces in `src/module.rs` (two passes: name/import resolution and
  local-binding-path collection, each gaining a `Closure` arm that resolves
  the parameter/return type annotations and recurses into the body with the
  closure's parameters added as new locals, on top of the enclosing locals —
  mirroring how `Head::For` already introduces a loop variable) and
  `src/typecheck.rs` (the value-position `walk_kind` match gains a `Closure`
  arm that reports a "この版では未対応です"-style diagnostic and poisons,
  without descending into the body — deferring free-variable/capture
  semantics entirely to CLO-020, unlike the `Index` stub which could safely
  synthesize its sub-expressions because they introduce no new bindings).
  This keeps `cargo build` green without doing CLO-020's job.
- Add a focused parser test proving the closure literal's `params`/`ret`
  fields, read through the same `Type`-conversion path a named function's
  `Sig` already uses, produce the identical `TypeKind::Callable` shape as
  the existing `fn(P1, P2 -> R)` type annotation — this is CLO-010's "closure
  値は既存の named 関数値と同じ型...として型検査へ渡る" completion condition,
  satisfied structurally (same `ast::Param`/`ast::Type` fields, no new type
  representation) rather than by wiring up real type-checking.
- Update `docs/grammar.md`'s "名前付き関数の値" section, which currently
  states "捕捉のある無名関数(クロージャ)はまだ無い" — replace with a
  description of the new literal's grammar and its current
  parses-but-unsupported-by-typecheck status, matching how the same section
  already documents `len()`/`xs[i]`.

## Capabilities

### New Capabilities
- `closure-literal-syntax`: The `fn(params -> ret) { body }` anonymous
  function literal grammar — parsing as a primary expression, mandatory
  parameter/return type annotations with no inference, its declared type
  shape matching the existing named-function-value `fn(P1, P2 -> R)` type
  representation, malformed-literal diagnostics (missing type annotations,
  missing braces), and non-interference with existing named `fn` items,
  `fn(...)` type annotations, and second-class `{ ... }` blocks.

### Modified Capabilities
- なし。既存の named 関数値・第二級ブロックの構文・挙動は変更しない。

## Impact

- `src/ast.rs`: add `ExprKind::Closure { params: Vec<Param>, ret:
  Option<Type>, body: Vec<Expr> }`.
- `src/parse.rs`: factor `sig()`'s parameter/return parsing into a shared
  helper; add a `Tok::Fn` case to `primary()`; new focused parser tests and
  dump-style coverage for zero/multi-parameter closures, omitted return
  type, malformed literals (missing param type, missing `->` target,
  missing `{`/`}`), and unchanged parsing of every existing `fn` item and
  `fn(...)` type-annotation fixture.
- `src/module.rs`: two existing exhaustive matches over `ast::ExprKind` gain
  a `Closure` arm that resolves parameter/return type names and recurses
  into the body with an extended locals set.
- `src/typecheck.rs`: `walk_kind`'s exhaustive match gains a `Closure` arm
  that reports an unsupported-construct diagnostic and poisons; no other
  typecheck behavior changes.
- `docs/grammar.md`: document the closure literal production and update the
  stale "クロージャはまだ無い" sentence.
- `tests/cli.rs`: CLI-level diagnostic tests for malformed closure literals
  (missing parameter type annotation, unclosed body) reporting a source
  span.
- No changes to `src/hir.rs`, `src/eval.rs`, `src/wasm*.rs`, or any runtime
  behavior — this task is grammar/type-representation-only. Interpreter and
  Wasm differential execution and Wasm byte-determinism are not applicable
  (no runtime path is touched, no fixture uses a closure literal); they are
  exercised again once CLO-060/070 wire `Closure` into checked HIR.
