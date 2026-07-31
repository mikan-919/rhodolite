## ADDED Requirements

### Requirement: Payload enums are owned values
A payload enum SHALL own the payload of its active variant. Moving, cloning, and
dropping the enum SHALL respectively transfer, structurally duplicate, or
release the active payload without implicit sharing.

#### Scenario: Payload ownership
- **WHEN** an owned value constructs a payload variant
- **THEN** ownership of that value enters the enum

### Requirement: Match mode applies to the whole scrutinee
`match value` SHALL shared-borrow a non-Copy scrutinee and bind payloads as shared
borrows. `match &mut value` SHALL exclusively borrow the scrutinee and selected
payload. `match move value` SHALL consume the whole enum and bind the selected
payload as owned. Pattern-level partial moves SHALL not be supported.

#### Scenario: Shared match preserves scrutinee
- **WHEN** a payload enum is matched without a modifier
- **THEN** arms borrow payloads and the enum remains usable after the match

#### Scenario: Consuming match
- **WHEN** a payload enum is matched with `move`
- **THEN** the selected payload becomes owned by its arm, unselected contents are dropped, and the source enum is unusable afterward

#### Scenario: Pattern-level move is rejected
- **WHEN** source writes a move modifier on an individual payload binding
- **THEN** parsing rejects the unsupported partial-move form
