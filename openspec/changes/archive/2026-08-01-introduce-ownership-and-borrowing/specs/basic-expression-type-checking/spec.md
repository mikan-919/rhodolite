## ADDED Requirements

### Requirement: Structural equality borrows compound operands
Equality for owned structs, payload enums, optionals, strings, arrays, and
indirect values SHALL compare their structure without moving or cloning either
operand. Comparison SHALL short-circuit and SHALL be available only when every
compared component is equality-comparable.

#### Scenario: Array equality preserves operands
- **WHEN** two arrays are compared with `==`
- **THEN** their lengths and elements are compared through shared borrows and both arrays remain owned by their callers

#### Scenario: Non-comparable component
- **WHEN** equality is applied to a composite containing a non-comparable value
- **THEN** checking rejects the equality expression

### Requirement: Clone is explicit and structural
`value.clone()` SHALL produce an owned structural clone only for cloneable data.
It SHALL recursively clone structs, active enum payloads, present optionals,
strings, arrays, and indirect values. No ordinary assignment, argument, return,
or comparison SHALL implicitly invoke clone.

#### Scenario: Deep string and array clone
- **WHEN** a struct containing a string and array is cloned
- **THEN** the clone owns independent buffers with equal contents

#### Scenario: Mutable borrow cannot be cloned
- **WHEN** code attempts to clone `&mut T`
- **THEN** checking rejects the operation
