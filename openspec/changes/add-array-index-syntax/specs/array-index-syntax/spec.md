## Purpose

Lets source code write a `len()` call and an `xs[i]` subscript so later
milestones (IDX-020 type checking/ownership, IDX-030/040 runtime contracts)
have syntax to attach their semantics to, without changing how any existing,
non-subscript program parses.

## ADDED Requirements

### Requirement: `len()` parses as an ordinary method call
`xs.len()` SHALL parse using the existing method-call grammar (a `Call` whose
callee is a `Field` projection), the same production already used for
`xs.push(y)`. No dedicated `len` syntax, keyword, or property-style `xs.len`
form SHALL be introduced.

#### Scenario: `len()` call parses
- **WHEN** the source contains `xs.len()`
- **THEN** parsing succeeds and produces the same AST shape as any other
  zero-argument method call, e.g. `xs.push(y)`

### Requirement: `xs[i]` parses as a postfix subscript expression
The parser SHALL accept `expr '[' expr ']'` as a postfix operation on any
expression, at the same precedence level and left-to-right chaining as
`.field`, `(args)`, and `::path`. The bracketed expression MAY be any valid
expression, including a nested subscript, a call, or a binary expression. The
resulting expression SHALL be usable anywhere an expression is valid: as a
statement, as an operand, as a method receiver, and as a function call
argument.

#### Scenario: Simple subscript on a local
- **WHEN** the source contains `xs[i]`
- **THEN** parsing succeeds and produces a subscript expression with `xs` as
  the base and `i` as the index

#### Scenario: Subscript on an array literal
- **WHEN** the source contains `[1, 2, 3][0]`
- **THEN** parsing succeeds, matching how `[1, 2, 3]` already parses as an
  array literal with a trailing subscript applied to it

#### Scenario: Chained and mixed postfix operations
- **WHEN** the source contains `xs[i][j]`, `xs.get(i)[0]`, or `xs[i].field`
- **THEN** parsing succeeds and each postfix operation (subscript, call,
  field access) applies left-to-right to the result of the previous one, with
  no precedence conflict between subscript and the existing method-call or
  field-access syntax

#### Scenario: Arbitrary expression inside the brackets
- **WHEN** the source contains `xs[i + 1]` or `xs[f(y)]`
- **THEN** parsing succeeds and the bracketed expression parses with its own
  full expression grammar (unaffected by surrounding operator precedence
  outside the brackets)

### Requirement: `xs[i] = v` is a valid assignment target
`xs[i]` SHALL be usable as the left-hand side of the existing assignment
expression (the same production that already accepts `user.id = id`), with no
separate assignment grammar introduced for subscripts. `xs[i]` used anywhere
other than the left-hand side of `=` SHALL parse as an ordinary expression.

#### Scenario: Subscript assignment parses
- **WHEN** the source contains `xs[i] = v`
- **THEN** parsing succeeds and produces the same assignment expression shape
  already used for `user.id = id`, with the subscript expression as `target`
  and `v` as `value`

#### Scenario: Subscript is an ordinary expression outside assignment
- **WHEN** the source contains `xs[i]` as a statement, as a call argument, or
  on the right-hand side of `=`
- **THEN** parsing succeeds and produces the same subscript expression shape
  as any other use, with no special-casing tied to assignment

### Requirement: Malformed subscript syntax is rejected with a source span
A subscript that is missing its closing `]`, or otherwise does not form a
complete `'[' expr ']'`, SHALL fail to parse with a diagnostic that includes
the source position of the error.

#### Scenario: Missing closing bracket
- **WHEN** the source contains `xs[i` with no closing `]`
- **THEN** parsing fails with a diagnostic reporting the expected `]` and its
  source position

#### Scenario: Empty subscript
- **WHEN** the source contains `xs[]`
- **THEN** parsing fails with a diagnostic reporting that an expression was
  expected inside the brackets, at its source position

### Requirement: Non-subscript syntax is unaffected
Every existing, non-subscript construct (array literals, method calls, field
access, path expressions, struct literals, and existing assignment targets)
SHALL parse to the same AST and produce the same diagnostics as before this
capability existed.

#### Scenario: Existing programs parse unchanged
- **WHEN** a program contains no `[...]` subscript on an expression (array
  literals and array type annotations are unaffected, since they do not use
  the new postfix production)
- **THEN** its parsed AST and any parse diagnostics are identical to before
  this capability existed
