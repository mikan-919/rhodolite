## Why

Enum payloads can now be constructed and destructured, but every `match` must still spell out every variant even when several variants intentionally share one result. A final catch-all arm is the smallest remaining pattern extension and removes that duplication without taking on guards, nested patterns, or a general pattern language.

## What Changes

- Accept `_` as a `match` arm pattern that handles every subject variant not handled by an earlier qualified variant arm.
- Require the catch-all arm to be unique and last so arm selection and reachability remain obvious.
- Treat a `match` with a catch-all arm as exhaustive while preserving duplicate checks for qualified variant arms.
- Evaluate the catch-all body only when no qualified arm matches; it introduces no payload bindings.
- Include the catch-all body in result-type checking and conservative ambient-requirement analysis like every other arm.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `fieldless-enum-matching`: Extend match-arm syntax, exhaustiveness, runtime selection, typing, and lexical/ambient analysis with a final `_` catch-all arm.

## Impact

The change affects the match-arm AST and parser, enum match checking and result inference, requirement analysis, evaluator arm selection, grammar and overview documentation, and focused parser/type-checker/evaluator/requirement tests. It adds no dependency and does not change existing exhaustive matches.
