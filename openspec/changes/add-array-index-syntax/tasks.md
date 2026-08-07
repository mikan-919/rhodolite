## 1. AST and grammar

- [x] 1.1 Add `ExprKind::Index(Box<Expr>, Box<Expr>)` to `src/ast.rs`.
- [x] 1.2 Add a `Tok::LBracket` case to `postfix()`'s loop in `src/parse.rs`,
      parsing `[` `self.expr()` `]` (via `self.expect(&Tok::RBracket, "`]`")`)
      onto the current expression, at the same precedence tier as
      `.field`/`(args)`/`::path`.
- [x] 1.3 Update `docs/grammar.md` with the `postfix '[' expr ']'`
      production.
- [x] 1.4 Add focused parser tests in `src/parse.rs`: simple subscript
      (`xs[i]`), subscript on an array literal (`[1, 2, 3][0]`), chained and
      mixed postfix (`xs[i][j]`, `xs.get(i)[0]`, `xs[i].field`), arbitrary
      expressions inside brackets (`xs[i + 1]`, `xs[f(y)]`), and unchanged
      parsing of every existing non-subscript fixture (array literals,
      method calls, field access, struct literals).
- [x] 1.5 Add a focused parser test asserting `xs.len()` parses to the same
      `Call(Field(...), [])` shape as `xs.push(y)` (regression lock for
      IDX-Q1; no parser code changes for this).

## 2. Assignment target

- [x] 2.1 Add a focused parser test asserting `xs[i] = v` parses as
      `Assign { target: Index(xs, i), value: v }` via the existing `assign()`
      production, with no changes to `assign()` itself.
- [x] 2.2 Add a focused parser test asserting `xs[i]` parses as an ordinary
      expression (same shape as 1.4's simple-subscript case) when used as a
      statement, a call argument, or the right-hand side of `=`.

## 3. Diagnostics

- [x] 3.1 Add focused parser tests for malformed subscripts: missing closing
      `]` (`xs[i`) and an empty subscript (`xs[]`), each asserting a
      diagnostic with a source position.
- [x] 3.2 Add a `tests/cli.rs` case asserting the missing-`]` (or empty
      subscript) diagnostic's rendered text and span.

## 4. Downstream compile-plumbing (mechanical, no semantics)

- [x] 4.1 Add an `ExprKind::Index` arm to the expression name-resolution
      match in `src/module.rs`, recursing into base and index like the
      existing `Field`/`Call` arms.
- [x] 4.2 Add an `ExprKind::Index` arm to the local-binding-collection match
      in `src/module.rs`, recursing into base and index the same way.
- [x] 4.3 Add an `ExprKind::Index` arm to `walk_kind`'s value-position match
      in `src/typecheck.rs`: `synth` both sub-expressions, then report a
      "この版では未対応です"-style diagnostic and poison. Do not change
      `typecheck::assign`'s existing wildcard target arm.
- [x] 4.4 Add a focused `module.rs` test confirming a name inside either side
      of an `Index` (e.g. `xs[some_import::i]` or the base being an
      as-yet-undeclared local) is still resolved/reported the same way it
      would be inside a `Field`/`Call` expression.
- [x] 4.5 Add a focused `typecheck.rs` test confirming a program using
      `xs[i]` compiles (parses, resolves, and reaches type checking) and
      fails type checking with a "not supported yet" diagnostic, and that an
      unrelated error nested inside `xs` or `i` (e.g. an undeclared local) is
      still reported.

## 5. End-to-end verification

- [x] 5.1 Run the full existing test suite, `cargo fmt --check`, and
      warnings-as-errors Clippy; confirm no existing AST/typecheck/module
      test output changes for any non-subscript program.
- [x] 5.2 Confirm this task's scope is grammar-only: no interpreter, Wasm, or
      differential-execution behavior is added or changed (no fixture uses
      `[...]` subscript syntax yet), and Wasm byte-determinism is unaffected
      since no Wasm-generation code path is touched.
- [x] 5.3 Commit the verified, test-passing state as a single stable
      snapshot.
