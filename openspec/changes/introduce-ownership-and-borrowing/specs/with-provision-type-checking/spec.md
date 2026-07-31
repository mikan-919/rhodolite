## ADDED Requirements

### Requirement: Value provisions declare ownership mode
A value-form `with slot(value)` provision SHALL shared-borrow a bound provider by
default. `with slot(&mut value)` SHALL exclusively borrow a mutable provider, and
`with slot(move value)` SHALL transfer ownership into the provision scope. A
fresh temporary provider expression SHALL be owned by the provision scope
without a move modifier. Type-only provisions SHALL carry no runtime ownership.

#### Scenario: Shared provider
- **WHEN** a bound provider is supplied without a modifier
- **THEN** the `with` body may use only trait operations compatible with shared access and the owner remains after the block

#### Scenario: Mutable provider
- **WHEN** a mutable provider is supplied with `&mut`
- **THEN** the `with` body has exclusive provider access and outside access conflicts until the borrow ends

#### Scenario: Moved provider
- **WHEN** a bound provider is supplied with `move`
- **THEN** the provision scope owns and drops it and the source binding is unavailable afterward

#### Scenario: Temporary provider
- **WHEN** a constructor call directly supplies a provider
- **THEN** the provision scope owns the temporary without an explicit move marker
