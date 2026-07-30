## Context

See `proposal.md` for motivation and
`specs/fieldless-enum-matching/spec.md` for behavior. `MatchArm` currently
contains a `MatchPattern`, a body, and the whole-arm span. Every compiler pass
iterates the same arms, but the evaluator selects an exact variant before
falling back to `_`.

Payload bindings are reconstructed independently in the loader, checker,
requirement analyzer, and evaluator. A guard must see those bindings in all
four stages. ADR-0004 requires structural changes to be declared before
implementation; this design adds one optional expression field to `MatchArm`
and no new AST or runtime type.

## Goals / Non-Goals

**Goals:**

- Add guards without changing the representation of patterns or enum values.
- Give a guard the same payload-local scope in every compiler stage.
- Keep exhaustiveness decidable by inspection: conditional arms do not cover a
  variant, while the existing final `_` remains the only conditional fallback.
- Preserve source-aware diagnostics and conservative ambient inference.

**Non-Goals:**

- Multiple arms for one variant, guarded catch-all arms, or general ordered
  pattern matching.
- Nested, literal, OR, or named-payload patterns.
- Binding the whole subject or changing match result-type inference.

## Decisions

### 1. Store the guard on `MatchArm`

Add `guard: Option<Box<Expr>>` between `pattern` and `body`. A guard controls
whether the arm is selected, so it belongs to the arm rather than to
`MatchPattern::Variant`. The parser and checker ensure that
`MatchPattern::CatchAll` never carries one.

Putting the guard in `MatchPattern::Variant` would make the pattern tree own an
arbitrary executable expression and force generic pattern consumers to walk
code. A separate guarded-arm variant would duplicate `body` and `span`. The
optional field keeps existing arms source-compatible within the AST and is the
only structural change required by ADR-0004.

### 2. Parse `Variant(payload) if condition` before the existing body

The grammar becomes:

```ebnf
match_arm ::= module_path '::' ident payload_pattern?
              ('if' expr)? (':' simple_expr | '{' expr* '}')
            | '_' (':' simple_expr | '{' expr* '}')
```

After parsing a qualified pattern, consume optional `if` and parse its
condition with the same no-struct-literal condition parser used by `if` and
`while`; then parse the existing arm body. If `_` is followed by `if`, emit a
parse diagnostic at that token.

Reusing `if` avoids a keyword and reads consistently with existing heads. The
no-struct mode prevents the body's opening `{` from being consumed as a struct
literal. Guard validity otherwise remains a checking concern so the guard
expression retains its own span.

### 3. Payload locals cover the guard and body

The loader canonicalizes the variant and creates the payload-local scope
before resolving the guard and body. The type checker calls the existing
`arm_locals` once, checks the guard with those locals, then checks the body with
a clone so bindings created while checking one expression cannot leak into the
other. Path collection follows the same order.

The requirement analyzer scans the guard and body from separate clones of the
same payload-local set. This conservatively includes every guard's calls and
ambient uses even though runtime selection may skip it. Separate clones also
prevent a `let` inside a guard-like block, if such an expression parses, from
changing body scope.

### 4. A guarded variant does not establish exhaustiveness

`check_arms` keeps one `covered` set for duplicate detection and adds an
`unconditional` set for exhaustiveness. Every qualified arm enters `covered`;
only an arm without a guard enters `unconditional`. Without `_`, missing
variants are computed from `unconditional`.

The duplicate rule continues to reject a second arm for the same variant even
if one or both have guards. This deliberately avoids ordered
duplicate-variant semantics. Consequently, a guarded arm requires the existing
final `_` to handle its false case. Allowing an unguarded duplicate as a local
fallback would make source order meaningful for qualified arms and is deferred
with general ordered patterns.

### 5. Check inferable guards as `bool`

For each guard, run ordinary expression checking and then the existing
`require(..., bool, ...)` compatibility check with the guard's own span.
Known non-boolean conditions fail before evaluation; unknown types retain the
project's current incremental type-checking boundary and are checked by the
evaluator.

Rejecting every unknown guard statically would make guards stricter than
existing `if` conditions and expand this change into new inference. Silently
accepting known non-boolean guards would postpone an avoidable failure.

### 6. Evaluate an exact arm's guard before falling back

Evaluate the subject once and locate its one qualified arm as today. If it has
a guard, push the payload scope, evaluate the guard once, and:

- on `true`, evaluate the qualified body in that scope;
- on `false`, pop the scope and evaluate the final `_` body in a fresh empty
  arm scope;
- on a non-boolean value, return a diagnostic while the guard expression is
  the innermost evaluation boundary.

An unguarded exact arm still wins immediately. If no qualified arm exists, use
`_` directly. Static exhaustiveness makes “false guard with no `_`” unreachable
after checking, while the evaluator retains a clear no-matching-arm error as a
runtime safety net.

## Risks / Trade-offs

- [Risk] One arm-local scope must be reproduced consistently across multiple
  passes. → Add focused loader, checker, requirement, and evaluator tests where
  a payload name shadows an imported declaration or ambient slot.
- [Risk] The current evaluator's exact-arm-first helper hides the new
  fall-through step. → Isolate qualified-arm evaluation from catch-all
  evaluation and pin subject/guard/body evaluation counts.
- [Risk] Diagnostics may point at the whole arm instead of the condition. →
  Check and evaluate the guard as its own `Expr`, preserving its existing span.
- [Trade-off] A guarded arm cannot be followed by another arm for the same
  variant. → Keep the language's current uniqueness invariant and defer
  ordered duplicate arms until a concrete need justifies a broader pattern
  model.

## Migration Plan

1. Add the optional guard field and update existing arm constructors with
   `None`, preserving all current behavior.
2. Parse and resolve qualified-arm guards, then add static typing,
   exhaustiveness, and ambient analysis.
3. Add conditional runtime selection and source-aware failures.
4. Update grammar, overview, README if needed, and end-to-end coverage.
5. Run formatting, the complete Rust suite, canonical and missing-handler
   examples, and strict OpenSpec validation.

Existing source needs no migration. Reverting removes guarded source/tests and
the optional field; no persisted data or dependency rollback is involved.
