## Context

See `proposal.md` for motivation and
`specs/array-index-syntax/spec.md` for the contract. Confirmed empirically
against the current tree (`cargo run -- <file>`):

- `xs.len()` already parses today — it fails only at typecheck
  (`` `len` の呼び出し先が決まりません ``, a method-resolution error), because
  it is the pre-existing `Field` + `Call` production also used by
  `xs.push(y)` (MAP-075). IDX-Q1 requires no grammar change.
- `xs[0]` does not parse today: `postfix()` (`src/parse.rs`) has no case for
  a trailing `Tok::LBracket`, so it errors with
  `` 1行に2つの式は書けません (次は LBracket) ``. `Tok::LBracket`/`RBracket`
  already exist (used for array literals `[1, 2, 3]` and array type
  annotations `[T]`), and the line-continuation tables `can_end_expr`/
  `can_start_expr` in `src/lex.rs` already include `RBracket`/`LBracket`
  (needed for array literals), so a same-line `xs[i]` is already lexed as a
  single logical line with no `Newline` token between `xs` and `[i]` — no
  lexer change is needed.
- `ast::ExprKind` (`src/ast.rs`) is matched exhaustively (no wildcard arm) in
  two places outside the parser: `typecheck::walk_kind`'s value-position
  match (`src/typecheck.rs`) and two passes in `src/module.rs` (expression
  name resolution, local-binding collection). Adding a new `ExprKind`
  variant fails to compile until each of these gains an arm — the same kind
  of mechanical, compiler-guided update MAP-010 made when it widened
  `Item::Impl`'s fields. `typecheck::assign`'s target-kind match already has
  a wildcard `_` arm (used today for e.g. `(a + b) = 5`), so no change is
  needed there for `xs[i] = v` to be rejected as an unsupported target.

## Goals / Non-Goals

**Goals:**
- Parse `xs[i]` as a postfix subscript expression per IDX-Q2/Q3/Q4's grammar
  shape (semantics come later), usable as an ordinary expression and, via the
  existing `Assign` production, as an assignment target per IDX-Q5.
- Keep every non-subscript program's parse output, diagnostics, and (via the
  mechanical exhaustive-match updates) type-check/module-resolution behavior
  byte-identical.
- Reject malformed subscripts with a source-positioned diagnostic, consistent
  with existing bracket-matching error messages.

**Non-Goals:**
- Type-checking `xs[i]`'s element type, borrow-checking the implicit shared
  or `&mut` borrow of `xs`, or range-checking `i` (IDX-020).
- Any interpreter or Wasm behavior for `len()` or `xs[i]` (IDX-030/040) —
  this task does not lower `Index` to `hir` at all.
- Any new syntax for `len()` — it already parses.

## Decisions

### 1. Add one AST variant, parsed inside the existing `postfix()` loop

`ast::ExprKind` gains `Index(Box<Expr>, Box<Expr>)`. `postfix()` already
threads a `loop` that stacks `.field`, `(args)`, `::path`, and `{fields}`
onto a base expression left-to-right; a `Tok::LBracket` arm is added to that
same loop:

```
} else if self.eat(&Tok::LBracket) {
    let index = self.expr()?;
    self.expect(&Tok::RBracket, "`]`")?;
    e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(index)), span: self.to(start) };
}
```

This gives subscript the same precedence and left-associative chaining as
every other postfix form for free — `xs[i][j]`, `xs.get(i)[0]`, and
`xs[i].field` all fall out of the same loop without a new precedence level.
The bracketed index is parsed with the full `self.expr()` grammar (matching
how call arguments and array-literal elements already accept any
expression), so `xs[i + 1]` and `xs[f(y)]` need no special casing.

Alternative considered: give subscript its own precedence tier between
`unary` and `postfix`, mirroring how some grammars separate "call" and
"index" tiers. Rejected — this codebase already treats every trailing
`.field`/`(args)`/`::path` as one uniform postfix tier with no precedence
distinctions between them; introducing a separate tier for `[...]` alone
would be an arbitrary asymmetry with no requirement (IDX-000) motivating it.

### 2. No new assignment grammar — `xs[i] = v` reuses `assign()` unchanged

`assign()` already parses `self.coalesce()` (which bottoms out through
`postfix()`) as `lhs`, then wraps it in `ExprKind::Assign { target: lhs, ... }`
if `=` follows, with only one target-shape check today (rejecting
`OptionalField` as read-only). Since `Index` becomes a possible shape of
`postfix()`'s result, `xs[i] = v` parses as `Assign { target: Index(xs, i),
value: v }` with zero changes to `assign()`. This is exactly IDX-010's
completion condition — "`xs[i] = v` は既存の代入文の産出に載せる" — satisfied
structurally rather than by adding a case.

### 3. Malformed subscripts fail through the existing `expect`/`err` machinery

A missing `]` is rejected by the same `self.expect(&Tok::RBracket, "`]`")?`
call other bracketed forms already use (array literals, `with slot<Type>`'s
`>` uses a different mechanism but array literals are the direct precedent),
which produces a source-positioned diagnostic via the parser's existing
error type. An empty `xs[]` is rejected because `self.expr()` itself fails
on `]` (no valid expression starts there) with the parser's normal "expected
an expression" diagnostic — no bespoke "empty subscript" message is added.

Alternative considered: a dedicated diagnostic for `xs[]` naming the
subscript context explicitly (e.g. "添字が空です"). Rejected — the generic
"unexpected token, expected expression" diagnostic already carries a source
span and is consistent with how every other empty-required-expression
position in this grammar (e.g. a missing call argument) is already reported;
a bespoke message would be one more string to keep in sync for no behavioral
gain the spec requires.

### 4. Downstream exhaustive matches get a minimal, inert arm

`ExprKind::Index` needs an arm in the three exhaustive matches identified in
Context, added mechanically:

- `src/module.rs` (expression name resolution): recurse into base and index,
  exactly like the existing `Field`/`Call` arms (`resolve_expr(base, ...)`
  then `resolve_expr(index, ...)`), so names inside either sub-expression are
  still resolved against imports/locals.
- `src/module.rs` (local-binding collection, the second exhaustive match):
  same recursive shape, mirroring `Field`/`Call`'s existing arms.
- `src/typecheck.rs` (`walk_kind`'s value-position match): synthesizes both
  sub-expressions with `synth(...)` (so any *other* error nested inside `xs`
  or `i` — an unresolved name, a bad call — is still reported, matching how
  `assign()`'s wildcard target arm already treats an unsupported shape) and
  then reports a single "この版では未対応です"-style diagnostic and poisons,
  reusing the wording pattern already used for "参照型は集約に置けない" in
  this file rather than inventing new phrasing. This is intentionally inert:
  it does no type checking, does no borrow accounting, and is fully replaced
  by IDX-020's real implementation, which will delete this stub arm's body.

Because `typecheck::assign`'s target match already falls through to its
wildcard `_` arm for any unhandled target shape, `xs[i] = v` needs no
`typecheck.rs` change beyond the one `walk_kind` arm above — the wildcard
arm's existing "この左辺には代入できません" message applies to it exactly as
it does today to any other not-yet-supported target. (Since `assign()`'s
wildcard also calls `synth(target, ...)`, evaluating `xs[i]` as a value one
sees both the `Index` stub's "not supported" diagnostic and the wildcard's
"cannot assign" diagnostic for the same statement; this double diagnostic is
accepted as a temporary artifact of the IDX-010/IDX-020 split, since IDX-010
does not test typecheck-level output for indexing — only IDX-020 does.)

Alternative considered: make the `walk_kind` stub silently poison with no
diagnostic, to avoid the double-diagnostic overlap with `assign()`'s
wildcard. Rejected — every other not-yet-connected-but-reachable expression
path in this codebase reports a diagnostic rather than silently poisoning
(poisoning without a diagnostic risks a confusing downstream "value used
after an unreported failure" state); a redundant diagnostic on an
unsupported construct is a smaller cost than a silent one.

### 5. `len()` needs no grammar change, but gets a locking regression test

Since `xs.len()` already parses via the pre-existing method-call grammar,
this task adds no code for it. It does add a focused parser test asserting
`xs.len()` parses to the same `Call(Field(...), [])` shape as `xs.push(y)`,
so the spec's `len()` requirement has automated coverage and any future
change to method-call parsing that broke `len()` specifically would be
caught.

## Risks / Trade-offs

- [Adding `ExprKind::Index` forces edits in `module.rs`/`typecheck.rs` even
  though this task is scoped to grammar] → Unavoidable given exhaustive
  matching (Decision 4); kept to the minimum recursive/stub shape so
  IDX-020 replaces exactly one match arm's body, not a new pass.
- [The double diagnostic on `xs[i] = v` (Decision 4) could look like a bug to
  a user before IDX-020 lands] → Accepted as temporary; IDX-010's own test
  scope is parser-level only (per ROADMAP's 検証方法), and IDX-020's arrival
  replaces the stub with the real assignment contract, removing the overlap.
- [`self.expr()` inside `[...]` accepts the full grammar, including a
  top-level `=` via `assign()`'s recursive call chain] → Not reachable in
  practice: `xs[i]` is parsed via `postfix()`, which is below `assign()` in
  the precedence chain, so by the time `postfix()` calls back into
  `self.expr()` for the bracketed index, that nested `self.expr()` call
  independently starts its own `assign()` → ... → `postfix()` descent and
  would only see `=` if the index expression itself contained one (e.g.
  `xs[y = 1]`, which is already how e.g. call arguments and array-literal
  elements behave today — consistent existing behavvior, not a new risk).

## Migration Plan

1. Add `ExprKind::Index` and the `postfix()` case, with `docs/grammar.md`
   updated in the same commit.
2. Add the mechanical `module.rs` (x2) and `typecheck.rs` (x1) match arms
   needed to keep the build green, per Decision 4.
3. Add focused parser tests (precedence/chaining/malformed-subscript spans,
   `len()` regression) and a `tests/cli.rs` diagnostic test for a malformed
   subscript; run the full suite, `cargo fmt --check`, and warnings-as-errors
   Clippy.
4. Commit the verified, test-passing state as a single stable snapshot.
5. Rollback is a plain revert: no runtime path, persisted data, or public ABI
   is touched, and no existing fixture uses `[...]` subscript syntax.
