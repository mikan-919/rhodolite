## 1. Spans identify their source

- [x] 1.1 Add a source identifier to `lex::Span`, take it as a parameter of `lex::lex`, and stamp it on every token; keep a zero-id entry point so existing lexer and parser tests read unchanged.
- [x] 1.2 Retain each module's path and text during loading, assign distinct source identifiers, expose them from `LoadedProgram`, and test that spans from a merged multi-module program resolve to the file they were lexed from.

## 2. Diagnostic value

- [x] 2.1 Introduce the diagnostic struct carrying message, optional span, optional label, optional help, and related diagnostics, with a constructor for the span-less case.
- [x] 2.2 Move lexer and parser errors onto it with the byte position they already know, replacing `ParseError`'s line-only rendering while keeping its message text.
- [x] 2.3 Move module-loading diagnostics onto it, spanning the `use` declaration for unresolvable modules and omitting the span for entry-path and read failures; cover both cases with tests.

## 3. Rendering

- [x] 3.1 Add the `miette` dependency and a CLI-side conversion that pairs a diagnostic with its module's source, rendering path, line, column, excerpt, and span extent, with related entries rendered against their own file.
- [x] 3.2 Keep span-less diagnostics, evaluation errors, the requirement listing, and the exit codes rendering as they do today, and pin this with CLI tests.

## 4. Checker diagnostics

- [x] 4.1 Move every `typecheck` diagnostic onto the diagnostic value with the span of the expression or declaration it reports, leaving the message text unchanged.
- [x] 4.2 Span the match exhaustiveness diagnostics at the offending arm, and the missing-variant diagnostic at the match expression.
- [x] 4.3 Move the declaration and duplicate-slot diagnostics of `requirement` onto the diagnostic value with their declaration spans.
- [x] 4.4 Add checker tests asserting the reported span for a representative diagnostic of each shape: expression, declaration, arm, and match expression.

## 5. Reachability path by position

- [x] 5.1 Record the call span on requirement call sites and carry per-hop spans through requirement propagation, keeping the inferred-requirement listing byte-identical.
- [x] 5.2 Report an unsatisfied requirement with its use site as the primary span, one related entry per hop, and the existing arrow-separated chain as the help note.
- [x] 5.3 Test a direct unsatisfied use, a transitive one, and a path whose hops are declared in different modules.

## 6. Documentation

- [ ] 6.1 Describe the diagnostic shape and what spans are guaranteed in `docs/overview.md`, and update the reachability-path rendering in `docs/requirement-map.md`.
- [ ] 6.2 Record the `miette` dependency decision as an ADR, since adding a dependency is a structural decision under ADR-0004.
