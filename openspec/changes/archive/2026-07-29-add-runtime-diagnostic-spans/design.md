## Context

Every expression already carries a `Span { src, start, end }`, including expressions loaded from another module. Pre-execution stages return `Diag`, and `render::report` combines those diagnostics with `LoadedProgram.sources` to render the correct source excerpt. Evaluation is the remaining exception: `Flow::Error` contains only a `String`, and `main.rs` prints that string directly for both entry-point and test failures.

Runtime errors can originate either directly in the current expression (for example, a false `assert` or division by zero) or in a nested expression or called function. The design must retain the most specific failing expression rather than repeatedly replacing its location while the error unwinds.

## Goals / Non-Goals

**Goals:**

- Give every runtime error raised while evaluating an AST expression the span of the innermost expression that observed the failure.
- Preserve source identity across function calls and module boundaries.
- Reuse `Diag`, `LoadedProgram.sources`, and `render::report` for CLI output.
- Keep `return` as control flow rather than presenting it as a diagnostic.
- Preserve existing runtime error wording and successful output.

**Non-Goals:**

- Adding stack traces or call-path-related diagnostics.
- Converting `Flow` into one variant per runtime error kind or adding error codes.
- Removing runtime checks that are currently redundant with static checking.
- Changing syntax, evaluation order, or language semantics.
- Adding a dependency or changing the miette renderer.

## Decisions

### 1. Store `Diag` in `Flow::Error`

`Flow::Error` will carry `Diag` instead of `String`; `Flow::Return(Value)` remains unchanged. The shared `fail` helper will initially create an unlocated `Diag::msg`, and `Display` will continue to expose only its message so existing unit-test assertions and error text remain readable.

This keeps runtime failures on the same data path as every pre-execution stage without creating a second runtime-only diagnostic type. Keeping `String` plus a parallel span field was rejected because it would duplicate `Diag` and still require a conversion at every public evaluator boundary.

### 2. Attach a span once at the `eval` boundary

Expression evaluation will have a thin span-owning wrapper around the existing expression-kind dispatch. When the dispatch returns `Flow::Error` without a span, the wrapper attaches the current expression's span and a short label. If the diagnostic already has a span, the wrapper leaves it untouched.

Because recursive evaluation and called function bodies re-enter the wrapper, the innermost expression that first observes a failure attaches its location. Propagation through a call expression, block, head, or caller cannot overwrite it. This central boundary also covers future expression kinds without requiring every `fail(...)` site to remember a span.

Passing a span into every evaluator helper was rejected because it creates repetitive plumbing and makes omissions likely. Attaching the caller's span only in `main.rs` was rejected because it cannot identify a nested expression or a callee in another module.

Errors raised before any expression is evaluated, such as directly requesting an unknown interpreter entry name, may remain unlocated. The loaded CLI path resolves its entry before evaluation, so this exception is an evaluator API guard rather than a source-program runtime failure.

### 3. Render runtime diagnostics at the CLI boundary

The CLI runner will receive the loaded source table. A failed `main` or test will retain its existing contextual heading, then pass the single runtime `Diag` to `render::report`. Tests will continue after one test fails and retain the final success count.

This preserves the current separation: the evaluator constructs diagnostics but does not know miette, terminal formatting, or filesystem paths. Rendering directly inside the evaluator was rejected for the same dependency-boundary reason recorded in ADR-0007.

### 4. Verify location precision independently from terminal decoration

Evaluator tests will assert the diagnostic's span against the exact failing expression, including a nested failure and a failure inside a function from another source module. CLI coverage will assert that a runtime failure is rendered with its source excerpt and that the failing test name remains visible.

Testing the structured diagnostic separately avoids coupling all evaluator tests to miette's decoration, while one CLI-level check protects the user-visible integration.

## Risks / Trade-offs

- **[A helper can create an error outside `eval`]** → Keep the public no-expression guard unlocated, and require all source-program failures to propagate through the expression wrapper; cover call and nested-helper failures in tests.
- **[A caller overwrites a more precise callee span]** → Fill the span only when it is absent and test a failure inside a called function.
- **[stdout/stderr ordering can make a test heading appear after its diagnostic]** → Flush or consistently route the contextual failure heading before invoking the existing stderr renderer, and cover the combined CLI output.
- **[Changing `Flow::Error` breaks string-oriented tests]** → Preserve `Display` as message-only and update helper extraction centrally rather than rewriting expected message text.
