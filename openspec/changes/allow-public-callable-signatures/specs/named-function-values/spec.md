## MODIFIED Requirements

### Requirement: Indirect calls target statically known named functions
The system SHALL allow a local or parameter with a callable type to be invoked
using ordinary call syntax. Every successful indirect call SHALL resolve, for
the enclosing call specialization, to one named top-level function supplied by
the caller. The system SHALL reject a callable value placed in a mutable
local, field, enum payload, or array element. The system SHALL likewise
reject a callable value placed in a function return value, unless the
function is selected as a public export of the entry module, in which case
`callable-public-boundary` governs whether that specific return value is
accepted.

#### Scenario: Higher-order helper invokes its callback
- **WHEN** `apply` accepts `fn(int -> int)` and evaluates `f(value)` through its
  callable parameter
- **THEN** `apply(double, 21)` evaluates to `42` through the named `double`
  function

#### Scenario: Unsupported callable storage is rejected
- **WHEN** a program attempts to assign a callable value to a struct field,
  enum payload, array element, or mutable local, or attempts to return a
  callable value from a function that is not selected as a public export
- **THEN** type checking rejects the use before ownership checking or execution

#### Scenario: Public export return value is not rejected by this requirement
- **WHEN** a function selected as a public export of the entry module
  declares a callable return type
- **THEN** this requirement does not reject the return position; the
  `callable-public-boundary` capability governs whether the declared type and
  the value that function actually returns are accepted
