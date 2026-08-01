# total-static-type-checking

## Purpose

Define the whole-program guarantee that successful static checking leaves no expression type, callable reference, named type, declaration target, or assignment target unresolved before analysis and evaluation.

## Requirements

### Requirement: Successful checking is total
The checker SHALL assign a concrete language type to every value-producing expression in every loaded function, method, and test body. It SHALL classify control-flow expressions that do not produce a value separately from value types. If either classification is impossible, checking SHALL fail before requirement analysis or evaluation.

#### Scenario: Every loaded body is checked
- **WHEN** a loaded declaration contains an expression whose type cannot be determined
- **THEN** checking fails even when that declaration is not called at runtime

#### Scenario: Control flow exits without a value
- **WHEN** a branch exits through `return`
- **THEN** the checker treats that branch as non-value-producing without inventing a user-visible type

#### Scenario: Successful program reaches later stages
- **WHEN** checking succeeds
- **THEN** requirement analysis and evaluation may assume that every reachable value-producing expression has a concrete type

### Requirement: Successful checking resolves every call
The checker SHALL resolve every direct call, method call, and associated-function call in every loaded body to exactly one declared signature. An unknown receiver, absent target, ambiguous target, or incompatible receiver form SHALL fail checking.

#### Scenario: Direct call resolves
- **WHEN** a direct function call names one loaded function
- **THEN** checking records a successful resolution to that declaration

#### Scenario: Member call cannot resolve
- **WHEN** the receiver type or selected member cannot be determined
- **THEN** checking fails before evaluation

#### Scenario: Ambiguous member call
- **WHEN** more than one declaration remains eligible for a call
- **THEN** checking fails with an ambiguity diagnostic

### Requirement: Unknown is an implementation state, not a successful result
The checker MAY use an internal unknown state while traversing expressions and collecting diagnostics, but SHALL NOT report overall success while any expression or call remains unknown. Unknown SHALL NOT be a user-visible dynamic or wildcard type.

#### Scenario: Earlier error causes an unknown dependent expression
- **WHEN** an expression cannot be typed because one of its inputs already produced a diagnostic
- **THEN** checking may suppress a redundant dependent diagnostic but still fails overall

#### Scenario: Unknown remains without an earlier cause
- **WHEN** checking finishes with an unknown expression that has no earlier diagnostic explaining it
- **THEN** checking emits a diagnostic for that expression

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

### Requirement: Type failures remain source-positioned
Every diagnostic introduced to close a previously deferred type or call resolution SHALL identify the source expression whose type or target could not be determined.

#### Scenario: Unknown expression in another module
- **WHEN** an unresolved expression occurs in a loaded dependency module
- **THEN** the diagnostic renders against that module and spans the unresolved expression

### Requirement: Successful checking closes ownership safety
After ordinary name and type resolution, the checker SHALL successfully classify
every value access as Copy, move, shared borrow, mutable borrow, or owned
construction and SHALL close all move, provenance, and exclusivity obligations
in every loaded body before requirement analysis, interpretation, or Wasm
support checking begins.

#### Scenario: Uncalled body violates ownership
- **WHEN** an uncalled loaded function contains a use after move
- **THEN** whole-program checking fails before execution

#### Scenario: Later stages receive checked ownership
- **WHEN** checking succeeds
- **THEN** requirement analysis, interpretation, and Wasm support checking may assume every access has a resolved ownership mode
