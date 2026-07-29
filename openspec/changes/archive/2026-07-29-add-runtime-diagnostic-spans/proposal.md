## Why

Rhodolite's pre-execution diagnostics point to source excerpts, but failures that can only occur while evaluating a program are still printed as unlocated strings. Now that all earlier compiler stages share `Diag` and the CLI has one miette renderer, runtime failures should use the same diagnostic path so users can see the expression that actually failed.

## What Changes

- Preserve a source span when an evaluated expression produces a runtime error, without replacing a more specific span already attached by a nested expression.
- Represent evaluator failures as `Diag` values while keeping `return` as non-error control flow.
- Render a failing `main` or test through the existing diagnostic renderer and loaded source table.
- Keep runtime error messages and successful program/test output behavior stable.

## Capabilities

### New Capabilities

- `runtime-diagnostic-spans`: Runtime failures identify the innermost failing source expression and are rendered with the same source-aware diagnostic pipeline as pre-execution failures.

### Modified Capabilities

None.

## Impact

- `src/eval.rs`: evaluator error representation, propagation, and span attachment.
- `src/main.rs`: runtime failure reporting and access to loaded sources.
- Evaluator and CLI tests: source selection, nested failure precision, and test-failure rendering.
- No syntax, language semantics, public dependency, or successful-output changes.
