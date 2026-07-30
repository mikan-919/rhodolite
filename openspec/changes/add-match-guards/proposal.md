## Why

`match` can destructure enum payloads and provide a final catch-all, but an arm
cannot select a value based on both its variant and a boolean condition. Guards
are the next smallest pattern extension: they express conditional selection
without introducing nested, literal, or OR patterns.

## What Changes

- Accept an optional `if <condition>` after a qualified enum-variant pattern.
- Make payload bindings visible to the guard as well as the arm body.
- Check every inferable guard condition against `bool` and retain the existing
  runtime check for conditions outside the current inference boundary.
- Evaluate a guard only after its variant matches; a false guard falls through
  to the final catch-all arm.
- Keep one qualified arm per variant and prohibit guards on `_`, avoiding
  ordered duplicate-variant arms in this change.
- Treat guarded variant arms as conditional rather than exhaustive, so every
  variant still needs an unguarded qualified arm or the existing final `_`.
- Include guard expressions in module loading, type checking, ambient
  requirement inference, runtime diagnostics, and documentation.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `fieldless-enum-matching`: Extend match-arm syntax, checking, exhaustiveness,
  evaluation, and analysis with boolean guards on qualified variant patterns.

## Impact

The change affects the match-arm AST and parser, module loader, enum match
checker and result inference, requirement analysis, evaluator selection,
grammar and overview documentation, and focused compiler/CLI tests. It adds no
dependency and preserves all existing unguarded matches.
