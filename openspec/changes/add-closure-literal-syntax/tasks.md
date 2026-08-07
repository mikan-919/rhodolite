## 1. AST and grammar

- [ ] 1.1 Add `ExprKind::Closure { params: Vec<Param>, ret: Option<Type>,
      body: Vec<Expr> }` to `src/ast.rs`.
- [ ] 1.2 Extract `sig()`'s parameter-list-then-optional-return parsing
      (`src/parse.rs`) into a shared `fn fn_params_and_ret(&mut self) ->
      PResult<(Vec<Param>, Option<Type>)>` helper; update `sig()` to call it
      with no behavior change.
- [ ] 1.3 Add a `Tok::Fn` case to `primary()` in `src/parse.rs`: `self.bump()`,
      `self.expect(&Tok::LParen, "`(`")?`, `fn_params_and_ret()`,
      `self.expect(&Tok::RParen, "`)`")?`, `self.block()?`, producing
      `ExprKind::Closure`.
- [ ] 1.4 Update `docs/grammar.md`'s "名前付き関数の値" section: add the
      closure literal production and replace the stale "捕捉のある無名関数
      (クロージャ)はまだ無い" sentence with a description of the new
      parses-but-not-yet-type-checked literal, matching how the same
      section documents `len()`/`xs[i]`.
- [ ] 1.5 Add focused parser tests in `src/parse.rs`: zero-parameter closure
      (`fn(-> int) { 1 }`), multi-parameter closure
      (`fn(a: int, b: int -> int) { a + b }`), omitted return type
      (`fn(x: int) { x }` treated as `unit`), closure as a `let` initializer,
      closure as a call argument, and unchanged parsing of every existing
      non-closure fixture (named `fn` items, `fn(P1, P2 -> R)` type
      annotations, second-class `{ ... }` blocks).

## 2. Type-shape lock

- [ ] 2.1 Add a focused parser test proving a closure literal's `params`/
      `ret` fields produce the same `ast::TypeKind::Callable` shape as an
      equivalent named function's `fn(P1, P2 -> R)` type annotation
      (design.md Decision 5) — parse both, extract each side's parameter/
      result types, and assert equality.

## 3. Diagnostics

- [ ] 3.1 Add focused parser tests for malformed closure literals: missing
      parameter type annotation (`fn(x -> int) { x }`), missing closing `)`
      after the parameter list, and an unclosed body (`fn(x: int -> int) {
      x`), each asserting a diagnostic with a source position and matching
      the wording named-function parsing already produces for the identical
      omission.
- [ ] 3.2 Add a `tests/cli.rs` case asserting the unclosed-body (or missing
      parameter type) diagnostic's rendered text and source-position
      excerpt.

## 4. Downstream compile-plumbing (mechanical, minimal semantics)

- [ ] 4.1 Add an `ExprKind::Closure` arm to `resolve_expr` in `src/module.rs`:
      resolve each parameter's type and the return type via `resolve_type`
      (mirroring `resolve_sig_types`), then recurse into `body` with a
      cloned `locals` set extended by the parameter names (mirroring
      `Head::For`'s `inner_locals` pattern).
- [ ] 4.2 Add the matching `ExprKind::Closure` arm to `collect_expr_paths`
      in `src/module.rs`, same shape.
- [ ] 4.3 Add an `ExprKind::Closure` arm to `walk_kind`'s value-position
      match in `src/typecheck.rs`: report a "この版では未対応です"-style
      diagnostic and `poison()`, without descending into `params`, `ret`,
      or `body` (design.md Decision 4).
- [ ] 4.4 Add a focused `module.rs` test confirming a closure body can
      reference its own parameter (no false "unresolved module reference"
      diagnostic) and can reference an enclosing local without a new
      diagnostic being introduced by this change.
- [ ] 4.5 Add a focused `typecheck.rs` test confirming a program using a
      closure literal (e.g. `let g = fn(x: int -> int) { x }`) compiles
      through parsing and module resolution and fails type checking with a
      "not supported yet"-style diagnostic naming the closure literal.

## 5. End-to-end verification

- [ ] 5.1 Run the full existing test suite, `cargo fmt --check`, and
      warnings-as-errors Clippy; confirm no existing AST/module/typecheck
      test output changes for any program without a closure literal.
- [ ] 5.2 Confirm this task's scope is grammar/type-representation-only: no
      interpreter, Wasm, or differential-execution behavior is added or
      changed (no fixture uses a closure literal yet), and Wasm
      byte-determinism is unaffected since no Wasm-generation code path is
      touched.
- [ ] 5.3 Commit the verified, test-passing state as a single stable
      snapshot.
