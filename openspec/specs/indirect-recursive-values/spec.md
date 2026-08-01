# indirect-recursive-values

## Purpose

Allows finite uniquely owned recursive data without exposing `Box<T>` wrappers,
while keeping every allocation-bearing recursive edge visible in the type declaration.

## Requirements

### Requirement: Recursive ownership edges are explicit
A struct field or enum payload MAY use the `indirect` modifier to store an owned
value through indirection while its source-visible value type remains `T` or
`T?`. Moving or dropping the enclosing value SHALL move or recursively drop the
indirect value under single ownership.

#### Scenario: Indirect optional field
- **WHEN** `struct Node` declares `indirect next: Node?`
- **THEN** the declaration has finite layout and each present child is uniquely owned

#### Scenario: Indirect enum payload
- **WHEN** a recursive enum variant declares an `indirect` payload of its enum type
- **THEN** finite values may terminate in a non-recursive variant and are dropped recursively

### Requirement: Inline recursive cycles are rejected
Every cycle in the graph of owned type containment SHALL contain at least one
`indirect` edge. A direct recursive cycle without indirection SHALL fail static
checking with a source-positioned diagnostic that identifies the cycle.

#### Scenario: Direct self recursion
- **WHEN** `Node` contains a direct owned `Node` field without `indirect`
- **THEN** checking rejects the infinite-size type

#### Scenario: Mutual recursion without indirection
- **WHEN** `Left` directly owns `Right` and `Right` directly owns `Left`
- **THEN** checking rejects the recursive containment cycle

#### Scenario: Mutual recursion is broken indirectly
- **WHEN** at least one edge in a mutual recursive cycle is declared `indirect`
- **THEN** the declarations have a finite layout
