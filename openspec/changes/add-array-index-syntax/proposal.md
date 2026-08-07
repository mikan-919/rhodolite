## Why

IDX-000 fixed the observable contract for `len()` and `xs[i]` (ROADMAP.md
Decisions IDX-Q1〜IDX-Q5): `len()` is a method call, `xs[i]` reads as a shared
borrow, out-of-range traps, indices are `int`-only, and `xs[i] = v` is a valid
assignment target. Before IDX-020 (type check / ownership) and IDX-030/040
(runtime contracts) can be built, the parser needs to accept this syntax at
all. Today `xs.len()` already parses as an ordinary method call (`Field` +
`Call`, unchanged since MAP-075's `push`), but `xs[i]` does not parse in any
position — `xs[0]` fails with `1行に2つの式は書けません (次は LBracket)`
because `postfix()` has no case for a trailing `[...]`.

## What Changes

- Add `ExprKind::Index(Box<Expr>, Box<Expr>)` to `src/ast.rs` and parse a
  trailing `[expr]` in `postfix()` (same loop as `.field`, `(args)`, `::path`,
  `{fields}`), so `xs[i]` parses as a place expression usable anywhere an
  expression is expected, including as the target of the existing `Assign`
  production (`xs[i] = v` needs no new assignment grammar — `assign()`
  already accepts any `postfix()` result as `target`).
- Confirm and lock in with tests that `xs.len()` already parses today via the
  existing method-call grammar (`Field` + `Call`) — no grammar change needed
  for IDX-Q1.
- Reject a missing `]` (and other malformed subscripts) with a
  source-positioned diagnostic, consistent with existing bracket/paren error
  reporting (`expect(&Tok::RBracket, ...)`).
- Add the minimal exhaustive-match plumbing this new `ExprKind` variant forces
  in `src/module.rs` (name resolution recurses into base and index) and
  `src/typecheck.rs` (the value-position `walk_kind` match gains an `Index`
  arm that type-checks neither operand's semantics — it synthesizes both sub-
  expressions and reports a "この版では未対応です"-style diagnostic, mirroring
  the existing "参照型は集約に置けない" stub pattern). This keeps `cargo
  build` green without doing IDX-020's job; real type checking, borrow
  semantics, and range-check contracts land in IDX-020/030/040. The existing
  wildcard arm in `typecheck::assign` already rejects `xs[i] = v` as an
  unsupported target with no change needed there.
- No lexer change: `Tok::LBracket`/`Tok::RBracket` and the existing line-
  continuation rules (`can_end_expr`/`can_start_expr`) already treat `[`/`]`
  correctly for a same-line postfix subscript; only `parse.rs` changes.

## Capabilities

### New Capabilities
- `array-index-syntax`: The `xs[i]` subscript grammar — parsing as a
  postfix expression, precedence/associativity relative to method calls and
  array literals, its use as an `Assign` target, malformed-subscript
  diagnostics, and non-interference with existing non-subscript syntax. Also
  documents (via a regression scenario) that `xs.len()` already parses via
  the pre-existing method-call grammar.

### Modified Capabilities
- なし。既存の non-subscript 構文の spec・挙動は変更しない。

## Impact

- `src/ast.rs`: add `ExprKind::Index(Box<Expr>, Box<Expr>)`.
- `src/parse.rs`: `postfix()` gains a `[expr]` case; new focused parser tests
  and dump-style (`show_expr`/`Debug`) coverage for precedence, chaining
  (`xs[i][j]`, `xs.get(i)[0]`, `[1,2,3][0]`), and malformed-subscript spans.
- `src/module.rs`: two existing exhaustive matches over `ast::ExprKind`
  (expression name resolution and local-binding collection) gain an `Index`
  arm that recurses into both the base and index sub-expressions, matching
  how `Field`/`Call` already recurse.
- `src/typecheck.rs`: `walk_kind`'s exhaustive match gains an `Index` arm that
  synthesizes both operands (so nested errors are still reported) and poisons
  with a "not supported yet" diagnostic; no change to `assign()`'s existing
  wildcard target rejection.
- `docs/grammar.md`: document the `postfix '[' expr ']'` production.
- `tests/cli.rs`: CLI-level diagnostic test for a malformed subscript (e.g.
  missing `]`) reporting a source span.
- No changes to `src/hir.rs`, `src/eval.rs`, `src/wasm*.rs`, or any runtime
  behavior — this task is grammar-only. Interpreter/Wasm differential
  execution and Wasm byte-determinism are not applicable to this task (no
  runtime path is touched); they are exercised again once IDX-030/040 wire
  `Index` into checked HIR.
