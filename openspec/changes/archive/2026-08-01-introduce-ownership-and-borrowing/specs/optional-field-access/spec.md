## ADDED Requirements

### Requirement: Optional projection preserves ownership safety
Optional field access SHALL copy a Copy field and shared-borrow a non-Copy field
through the optional owner. The resulting borrow SHALL not outlive or conflict
with access to that owner. Optional projection SHALL NOT implicitly move or
clone a field.

#### Scenario: Non-Copy optional field
- **WHEN** optional projection reaches a present non-Copy field
- **THEN** it produces a borrow tied to the optional receiver's provenance

#### Scenario: Receiver mutation conflicts
- **WHEN** an optional-field borrow remains live and code mutates the optional receiver
- **THEN** ownership checking rejects the conflicting mutation
