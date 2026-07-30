## MODIFIED Requirements

### Requirement: Unknown provision types fail static checking
The checker SHALL require every `with slot(value)` provision for a declared slot to have a concrete inferred value type and SHALL compare that type with the slot trait before evaluation. A provision value whose type cannot be determined SHALL fail checking. A head name that is not a declared slot SHALL continue to use the existing undeclared-slot diagnostic.

#### Scenario: Provision value has no type
- **WHEN** a `with slot(value)` provision's value type cannot be determined
- **THEN** checking fails at the provision value instead of deferring to evaluation

#### Scenario: Head name is not a slot
- **WHEN** a `with` head names something that is not a declared slot
- **THEN** checking fails through the existing undeclared-slot diagnostic
