## Purpose

Defines which callable values may cross the public Wasm ABI boundary and
enforces the ambient-zero restriction the host cannot satisfy, so a public
signature's callable type is checked before any handle runtime exists.

## ADDED Requirements

### Requirement: Public signatures may declare named-function callable types
A function selected as a public export of the entry module SHALL be allowed
to declare a callable type `fn(P1, P2 -> R)` as a parameter type and as its
return type. The callable type SHALL follow the same outermost-only shape
rule as an ordinary callable parameter: it MUST NOT be nested inside another
type, marked optional, or written behind a reference, whether it appears as
a parameter or as the return type.

#### Scenario: Public function declares a callable parameter
- **WHEN** a function selected as a public export declares a parameter with
  callable type `fn(int -> int)`
- **THEN** type checking accepts the parameter's declared type

#### Scenario: Public function declares a callable return type
- **WHEN** a function selected as a public export declares return type
  `fn(int -> int)`
- **THEN** type checking accepts the declared return type

#### Scenario: Nested callable in a public signature is rejected
- **WHEN** a function selected as a public export declares a parameter or
  return type where a callable type is optional, behind a reference, or
  nested inside an array or another aggregate
- **THEN** type checking rejects the declaration with a source span at that
  parameter or return type

### Requirement: A callable crossing the public boundary must resolve to one named function with zero ambient requirements
For every value-producing return path of a public export whose return type
is a callable type, the system SHALL determine the one named top-level
function that path returns. The system SHALL reject that public export,
with a source span at the offending return expression, when the path's
value is not a direct reference to a single statically known named
function, or when that function's own inferred ambient requirements
(evaluated with no callback bound) are not empty.

#### Scenario: Callable return value with zero ambient requirement is accepted
- **WHEN** a public export's callable-typed return path directly names a
  top-level function that requires no ambient slot
- **THEN** type checking accepts the public export

#### Scenario: Callable return value requiring an ambient slot is rejected
- **WHEN** a public export's callable-typed return path directly names a
  top-level function whose own inferred ambient requirements are not empty
- **THEN** compilation fails before Wasm generation with a source-spanned
  diagnostic naming the returned function and the ambient slot it requires

#### Scenario: Callable return value without one static target is rejected
- **WHEN** a public export's callable-typed return path does not resolve to
  a single statically known named function (for example, forwarding an
  unbound callable-typed parameter)
- **THEN** compilation fails before Wasm generation with a source-spanned
  diagnostic at the return expression

### Requirement: Closures cannot cross the public boundary
The system SHALL reject a closure literal used anywhere a public function's
callable-typed parameter or return value would need to accept it, with a
source span. Only named top-level function values are in scope for crossing
the public boundary; closures with a captured environment remain a Future
Direction.

#### Scenario: Closure literal returned from a public function is rejected
- **WHEN** a public export's body attempts to produce its callable-typed
  return value from a closure literal instead of a named top-level function
- **THEN** type checking rejects the closure literal with a source span
