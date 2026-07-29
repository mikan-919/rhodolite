## 1. Structured Evaluator Errors

- [ ] 1.1 Change `Flow::Error` and the shared failure helper to carry `Diag` while preserving message-only display and `return` control flow
- [ ] 1.2 Update evaluator test helpers to inspect structured runtime diagnostics without changing existing message expectations

## 2. Expression Span Preservation

- [ ] 2.1 Wrap expression dispatch so the first evaluated expression to observe an unlocated runtime error attaches its span and enclosing callers preserve it
- [ ] 2.2 Add evaluator tests for direct and nested failures, a failure inside a called function, and an unlocated pre-expression entry guard
- [ ] 2.3 Add module-backed coverage proving a callee failure retains the source identity and range of the module that owns the failing expression

## 3. CLI Rendering

- [ ] 3.1 Pass loaded sources into the execution runner and render failed `main` diagnostics through `render::report`
- [ ] 3.2 Render failed test diagnostics with their test names while continuing the suite and preserving the final result count
- [ ] 3.3 Add CLI coverage for source excerpts on main and test runtime failures and for unchanged successful output

## 4. Documentation and Verification

- [ ] 4.1 Update the runtime-diagnostic description and remaining-work map in `docs/overview.md`
- [ ] 4.2 Run formatting, the full test suite, and representative successful and runtime-failing CLI programs
- [ ] 4.3 Validate the OpenSpec change and confirm every runtime-diagnostic-spans scenario is covered
