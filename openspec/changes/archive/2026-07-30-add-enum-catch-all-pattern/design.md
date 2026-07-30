## Context

See `proposal.md` for motivation. A match arm is currently represented only by `MatchArm { enum_name, variant, bindings, body, span }`, so every arm necessarily names one qualified variant. The parser, module loader, type checker, requirement analyzer, and evaluator all inspect those fields directly.

The current checker builds a set of qualified variants and reports every declaration absent from that set. The evaluator searches for one matching qualified arm and errors if none exists. Result typing and requirement analysis already iterate over all arm bodies, while payload locals are derived from each arm's variant declaration.

ADR-0004 requires new structures and invariants to be declared before implementation. This change needs one new sum type to distinguish a qualified variant pattern from a catch-all pattern; no new runtime value or general pattern tree is needed.

## Goals / Non-Goals

**Goals:**

- Represent qualified variant and catch-all arms without sentinel strings or impossible field combinations.
- Keep qualified-arm payload binding and canonicalization behavior unchanged.
- Make uniqueness, final position, exhaustiveness, runtime fallback, type flow, and ambient analysis explicit and testable.
- Preserve all existing exhaustive match source unchanged.

**Non-Goals:**

- Binding the whole enum value or its payload from `_`.
- Guards, multiple conditional arms for one variant, nested/literal/OR patterns, or named payloads.
- General unreachable-arm analysis or warnings for a catch-all that covers no remaining variant.
- Changing match subject typing, result-type inference, or enum runtime representation.

## Decisions

### 1. Give `MatchArm` an explicit sum-typed pattern

Add a structure equivalent to:

```rust
enum MatchPattern {
    Variant {
        enum_name: String,
        variant: String,
        bindings: Vec<PatternBinding>,
    },
    CatchAll,
}
```

and replace the three variant-only fields on `MatchArm` with `pattern: MatchPattern`. The invariant is that only `Variant` carries payload bindings; `CatchAll` carries none.

Using `_` as a sentinel enum or variant name would let loader and checker code accidentally treat it as a declaration path. Adding `catch_all: bool` while retaining mandatory variant fields would create invalid states. A sum type makes each consumer handle both cases and keeps a future general pattern tree out of this change.

This is the sole new structure declared for ADR-0004.

### 2. Parse `_` only at the whole-arm pattern position

The grammar becomes:

```ebnf
match_arm ::= (module_path '::' ident payload_pattern? | '_')
              (':' simple_expr | '{' expr* '}')
```

The parser recognizes `_` before attempting the existing qualified path parse. `_` cannot take a payload binding list. Parser tests pin the new form and malformed `_ (...)` input. Ordering and uniqueness remain semantic checks rather than parser state so their diagnostics use the same arm spans and checking phase as variant exhaustiveness.

Alternatives such as `else` or `default` introduce a new keyword for behavior already conventionally expressed by `_`. Reusing `PatternBinding::Discard` for the whole arm is also rejected because discarding one payload position and matching every remaining variant are different grammatical levels.

### 3. A catch-all is unique, final, and sufficient for exhaustiveness

The checker walks arms in source order while tracking qualified variants and the first catch-all index. It preserves all existing validation for `Variant` patterns. For `CatchAll`, it reports a duplicate after the first and reports the first catch-all when any later arm makes it non-final.

Missing variants are reported only when no catch-all exists. A catch-all after explicit coverage of every variant is allowed: it is unreachable today, but the language has no general unreachable-code diagnostic and adding one is outside this change.

Allowing arms after `_` would either make them silently unreachable or require ordered pattern semantics that existing qualified arms do not need. Requiring `_` last leaves one obvious fallback and avoids laying groundwork for guards prematurely.

### 4. Runtime selection prefers the matching variant and then falls back

The evaluator evaluates the subject once, searches qualified arms for the subject's enum and variant, and uses the final catch-all only if no qualified arm matches. A qualified arm retains its payload arity check and arm-local bindings. A catch-all evaluates its body with a fresh empty arm scope and ignores the subject payload.

Even though static checking enforces final position, explicit variant-first selection keeps unchecked evaluator behavior deterministic and makes the intended fallback rule direct.

Binding the full subject to `_` is rejected: `_` consistently means “do not bind” in existing payload patterns. A future named whole-value pattern would be a separate syntax and change.

### 5. Generic arm-body passes handle catch-all without special semantics

Result inference and expected-type checking continue to iterate over every arm body. Their diagnostic labels render a qualified name for `Variant` and `_` for `CatchAll`.

Requirement analysis likewise scans every body conservatively. It adds payload locals only for a `Variant`; a `CatchAll` starts from the surrounding locals unchanged. Module loading canonicalizes only `Variant` paths and traverses every body.

This keeps catch-all behavior inside the match abstraction and avoids separate post-processing paths that could omit its type or ambient requirements.

## Risks / Trade-offs

- [Risk] Replacing direct `MatchArm` fields touches every compiler stage and many tests. → Introduce `MatchPattern` in the AST first, update exhaustive matches compiler-wide, and keep existing syntax tests green before adding `_`.
- [Risk] Duplicate and non-final catch-all errors could both fire for the same malformed arm list. → Define deterministic source-order diagnostics and pin their spans and messages in type-checker tests.
- [Risk] Ignoring payload in a catch-all may surprise users expecting `_` to bind the subject. → Keep `_` consistent with existing discard semantics and document that no name is introduced.
- [Trade-off] A redundant final `_` is accepted. → Defer unreachable-code policy until the language has a general model rather than inventing a match-only warning.

## Migration Plan

1. Introduce `MatchPattern` and mechanically move existing qualified-arm data into its `Variant` case across all compiler stages.
2. Add whole-arm `_` parsing and semantic validation for uniqueness, final position, and exhaustiveness.
3. Add evaluator fallback selection and verify result typing and requirement analysis across catch-all bodies.
4. Update grammar, overview, and focused integration coverage.
5. Run formatting, the complete Rust test suite, canonical and missing-handler examples, and strict OpenSpec validation.

Existing Rhodolite source needs no migration. Reverting the change consists of removing catch-all source/tests and collapsing the `Variant` case back into `MatchArm`; no persisted data or dependency rollback is involved.
