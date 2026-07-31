## ADDED Requirements

### Requirement: Arrays own their elements and buffer
`[T]` SHALL be a non-Copy owned mutable-length array whose owner controls its
element values and backing storage. Moving an array SHALL transfer that
ownership; cloning it SHALL allocate an independent buffer and structurally
clone its elements.

#### Scenario: Array move
- **WHEN** an array local moves to a new owner
- **THEN** its elements and buffer are not copied and the source becomes unusable

#### Scenario: Array clone
- **WHEN** an array is explicitly cloned
- **THEN** subsequent mutation of one array does not alter the other's buffer or elements

### Requirement: For loops select an ownership mode
`for item in array` SHALL shared-borrow the array and bind non-Copy elements as
shared borrows. `for item in &mut array` SHALL exclusively borrow each element
for its iteration. `for item in move array` SHALL consume the array and yield
owned elements, then release its buffer. An active array iteration SHALL forbid
conflicting structural mutation of the same array.

#### Scenario: Shared iteration preserves array
- **WHEN** a non-Copy array is iterated without a modifier
- **THEN** each element is read through a borrow and the array remains usable afterward

#### Scenario: Mutable iteration
- **WHEN** a mutable array is iterated with `&mut`
- **THEN** each loop binding permits exclusive element mutation without moving the array

#### Scenario: Consuming iteration
- **WHEN** an array is iterated with `move`
- **THEN** each element becomes owned by its iteration and the original array is consumed
