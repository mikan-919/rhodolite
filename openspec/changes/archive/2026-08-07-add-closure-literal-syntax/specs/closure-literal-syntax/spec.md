## Purpose

Lets source code write an anonymous, capture-carrying function literal
`fn(params -> ret) { body }` so later milestones (CLO-020 capture
resolution/type-checking, CLO-030 ownership, CLO-040 specialization,
CLO-060/070 execution) have syntax and a type representation to attach their
semantics to, without changing how any existing named function or
second-class block parses.

## ADDED Requirements

### Requirement: An anonymous function literal parses as a primary expression
The parser SHALL accept `'fn' '(' params? ')' block` as a primary expression,
where `params` is a comma-separated list of `name ':' type` pairs (identical
grammar to a named function's parameter list) optionally followed by
`'->' type`, and `block` is an ordinary brace-delimited body (identical
grammar to a named function's body). The literal introduces no name of its
own. The resulting expression SHALL be usable anywhere an expression is
valid: as a statement, as a `let` initializer, and as a function call
argument.

#### Scenario: Zero-parameter closure literal
- **WHEN** the source contains `fn(-> int) { 1 }` in an expression position
- **THEN** parsing succeeds and produces a closure literal with no
  parameters and an `int` return annotation

#### Scenario: Multi-parameter closure literal
- **WHEN** the source contains `fn(a: int, b: int -> int) { a + b }`
- **THEN** parsing succeeds and produces a closure literal with two
  parameters (`a: int`, `b: int`) and an `int` return annotation

#### Scenario: Closure literal with an omitted return annotation
- **WHEN** the source contains `fn(x: int) { let ignored = x }`, with no
  `->` clause
- **THEN** parsing succeeds and the closure's return type is treated as
  `unit`, the same rule a named function's omitted `->` already follows

#### Scenario: Closure literal used as a let initializer
- **WHEN** the source contains `let g = fn(x: int -> int) { x }`
- **THEN** parsing succeeds and `g`'s initializer is the closure literal
  expression

#### Scenario: Closure literal used as a call argument
- **WHEN** the source contains `apply(fn(x: int -> int) { x }, 1)`
- **THEN** parsing succeeds and the closure literal parses as an ordinary
  argument expression, the same position a named function value already
  parses in

### Requirement: Parameter and return type annotations are mandatory, never inferred
Every closure parameter SHALL require an explicit `: type` annotation, and a
present `->` SHALL require an explicit result type, using the same type
grammar as named function parameters and results. The parser SHALL NOT
attempt to infer a parameter or return type from an expected type at the use
site or from the body.

#### Scenario: Missing parameter type annotation is rejected
- **WHEN** the source contains `fn(x -> int) { x }`, omitting `x`'s `: type`
- **THEN** parsing fails with a diagnostic reporting the expected `:` and its
  source position, the same diagnostic a named function's parameter list
  already produces for the identical omission

#### Scenario: A closure with no return arrow is not treated as inferring one
- **WHEN** the source contains `fn(x: int) { x }`
- **THEN** parsing succeeds (no arrow is required), but the closure's
  effective return type is `unit` per the omitted-arrow rule above, not
  `int` inferred from the body's last expression

### Requirement: A closure literal's declared type is the same `fn(P1, P2 -> R)` shape as a named function value
The parameter types and return type read off a closure literal SHALL produce
the identical type representation the system already uses for a named
top-level function's callable value type (`fn(P1, P2 -> R)`), with no
separate closure-specific type introduced. This holds independent of
whether the closure's body ever type-checks.

#### Scenario: A closure's declared shape matches an equivalent named function's callable type
- **WHEN** a named function `fn add(a: int, b: int -> int) { a + b }` is
  referenced where a `fn(int, int -> int)` value is expected, and a closure
  literal `fn(a: int, b: int -> int) { a + b }` is written elsewhere in the
  same program
- **THEN** the closure literal's parameter and return type annotations
  produce the same type shape as the `fn(int, int -> int)` annotation used
  for the named function, parameter-for-parameter and result-for-result

### Requirement: Closure literal syntax does not change any other construct's parsing
Every existing named `fn` item, every existing `fn(P1, P2 -> R)` type
annotation, and every existing second-class `{ ... }` block SHALL parse to
the same AST and produce the same diagnostics as before this capability
existed. `fn` immediately followed by an identifier SHALL still parse as a
named function item wherever an item is expected; `fn` immediately followed
by `(` in a type-annotation position SHALL still parse as a `Callable` type
annotation; only `fn` immediately followed by `(` in an expression position
is newly accepted as a closure literal.

#### Scenario: Existing programs parse unchanged
- **WHEN** a program contains no anonymous `fn(...) { ... }` expression (named
  `fn` items, `fn(P1, P2 -> R)` type annotations, and bare `{ ... }` blocks
  are unaffected, since none of them use the new primary-expression
  production)
- **THEN** its parsed AST and any parse diagnostics are identical to before
  this capability existed

### Requirement: Malformed closure literal syntax is rejected with a source span
A closure literal missing its closing `)`, its opening or closing `{`/`}`,
or otherwise not forming a complete `'fn' '(' params? ')' block`, SHALL fail
to parse with a diagnostic that includes the source position of the error.

#### Scenario: Unclosed closure body
- **WHEN** the source contains `fn(x: int -> int) { x` with no closing `}`
- **THEN** parsing fails with a diagnostic reporting the missing `}` and its
  source position

### Requirement: Closure literal bodies are not yet type-checked
A program containing a closure literal SHALL parse and reach type checking,
but type checking SHALL reject the closure literal with a diagnostic stating
that it is unsupported in this version, without evaluating the body's
free-variable references, ownership, or ambient requirements. This
capability does not make any closure literal usable at runtime.

#### Scenario: A parsed closure literal fails type checking, not parsing
- **WHEN** a program containing a syntactically valid closure literal
  (e.g. `let g = fn(x: int -> int) { x }`) is compiled
- **THEN** parsing succeeds, and compilation fails at type checking with a
  diagnostic stating the closure literal is unsupported in this version
