## ADDED Requirements

### Requirement: Optionals own present values
`T?` SHALL own its present `T` value. Moving, cloning, and dropping an optional
SHALL respectively transfer, structurally duplicate, or release that present
value. An optional of a borrowed type SHALL remain unsupported in this version.

#### Scenario: Present value enters optional
- **WHEN** an owned `T` initializes `T?`
- **THEN** ownership of the value transfers into the optional

### Requirement: Coalescing follows its ownership mode
For Copy `T`, `optional ?? fallback` SHALL produce a copied `T`. For non-Copy
`T`, plain `optional ?? fallback` SHALL produce a shared borrow from the present
value or fallback. `move optional ?? fallback` SHALL consume the optional and
produce an owned present value, evaluating and taking the fallback only when
the optional is empty.

#### Scenario: Borrowed coalesce
- **WHEN** a non-Copy present optional is coalesced without `move`
- **THEN** the result borrows its content and the optional remains initialized

#### Scenario: Consuming coalesce
- **WHEN** a non-Copy optional is coalesced with `move`
- **THEN** the result is owned and the source optional is consumed

#### Scenario: Fallback remains short-circuited
- **WHEN** a consuming coalesce finds a present value
- **THEN** the fallback is not evaluated or moved
