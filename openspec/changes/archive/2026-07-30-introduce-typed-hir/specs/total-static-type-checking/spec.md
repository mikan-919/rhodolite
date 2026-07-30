## ADDED Requirements

### Requirement: Successful checking resolves every named type
The checker SHALL resolve every type annotation — struct field, enum payload, function, trait-method and implementation-method parameter and return, and local `let` annotation — to a builtin type or to one declared struct or enum. A name that matches no declaration SHALL fail checking, reported once per name.

#### Scenario: Annotation names no declaration
- **WHEN** a signature or field annotation names a type that is not declared and is not a builtin
- **THEN** checking fails before evaluation, even when the declaration is never called

#### Scenario: The same unknown name appears repeatedly
- **WHEN** one undeclared type name is written in several annotations
- **THEN** exactly one diagnostic is reported, positioned at the first occurrence

### Requirement: Successful checking resolves every declaration target
The checker SHALL resolve the trait named by every `effect` declaration to one declared trait, and the implementation target of every inherent `impl` to one declared struct. A target that resolves to nothing, or to a declaration of the wrong kind, SHALL fail checking.

#### Scenario: Slot names something that is not a trait
- **WHEN** `effect db: Nope` names no declared trait
- **THEN** checking fails, because no implementation could be selected for that slot

#### Scenario: Inherent implementation target is not a struct
- **WHEN** `impl Nope { ... }` names no declared struct
- **THEN** checking fails, and the method bodies are still checked for their own errors

### Requirement: Successful checking resolves every assignment target
The checker SHALL resolve the left-hand side of every assignment to one local binding or one declared struct field. Assigning to a slot, to a field the receiver's type does not declare, to a receiver that is not a struct, or to any other expression form SHALL fail checking.

#### Scenario: Assigning to a slot
- **WHEN** an assignment targets a slot name, whether declared by `effect` or introduced by `with`
- **THEN** checking fails instead of deferring the failure to run time

#### Scenario: Assigning to an undeclared field
- **WHEN** an assignment targets a field the receiver's declared type does not have
- **THEN** checking fails, preserving the promise that struct values reaching evaluation have their declared shape

#### Scenario: Assignment through an optional receiver
- **WHEN** the receiver is optional and its content type declares the field
- **THEN** the target resolves to that declared field and checking does not compare the assigned value against it
