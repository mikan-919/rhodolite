# array-type-checking

## Purpose

Define array type syntax, array literal typing under and without an expected
type, invariant array assignability, and the element type bound by `for`.

## Requirements

### Requirement: Array type syntax
The language SHALL accept `[T]` in every type annotation position where a named type is accepted, SHALL allow array element types to be nested or optional, and SHALL apply a trailing `?` to the completed array type.

#### Scenario: Array type in declarations
- **WHEN** a program declares a field, parameter, or return type as `[User]`
- **THEN** the parser represents it as an array whose element type is `User`

#### Scenario: Optional array and optional element are distinct
- **WHEN** a program uses `[User]?` and `[User?]`
- **THEN** the former denotes an optional array of `User` and the latter denotes an array of optional `User`

#### Scenario: Nested array type
- **WHEN** a program uses `[[int]]`
- **THEN** the parser represents it as an array whose element type is `[int]`

### Requirement: Array literal inference
The type checker SHALL infer `[T]` for a non-empty array literal when its inferable elements have the same type `T`, and SHALL diagnose conflicting known element types.

#### Scenario: Homogeneous literal
- **WHEN** the checker sees `[1, 2, 3]` without an expected type
- **THEN** it infers `[int]`

#### Scenario: Heterogeneous literal
- **WHEN** the checker sees `[1, true]`
- **THEN** it reports that the array elements have incompatible types

#### Scenario: Unknown element prevents complete inference
- **WHEN** an array literal contains an element whose type is not known and has no expected array type
- **THEN** the checker does not assign a known type to the array from later assignments

### Requirement: Contextual array literal checking
When an array literal occurs at a boundary with expected type `[T]`, the type checker SHALL check every element against `T` using the existing assignability rules. An empty array literal SHALL fit an expected array type but SHALL have no independently inferred element type.

#### Scenario: Empty array with expected type
- **WHEN** `[]` initializes a field declared as `[User]`
- **THEN** the checker accepts the literal as `[User]`

#### Scenario: Empty array without expected type
- **WHEN** `let xs = []` has no expected type
- **THEN** the checker leaves `xs` without a known type

#### Scenario: Element mismatch at expected boundary
- **WHEN** `[1, true]` is passed to a parameter declared as `[int]`
- **THEN** the checker reports the `bool` element as incompatible with `int`

#### Scenario: Optional element injection
- **WHEN** `[1, nil]` initializes a field declared as `[int?]`
- **THEN** the checker accepts both elements using the existing optional injection and `nil` rules

### Requirement: Invariant array assignability
The type checker SHALL consider two known array value types assignable only when their element types and array optionality match recursively, except that the existing optional injection MAY wrap the outer array value from `[T]` into `[T]?`.

#### Scenario: Matching array values
- **WHEN** a value of type `[User]` is assigned to a destination of type `[User]`
- **THEN** the checker accepts the assignment

#### Scenario: Element optionality is invariant
- **WHEN** a value of type `[User]` is assigned to a destination of type `[User?]`
- **THEN** the checker reports incompatible types

#### Scenario: Outer optional injection
- **WHEN** a value of type `[User]` is passed to a parameter of type `[User]?`
- **THEN** the checker accepts the value using the existing non-optional-to-optional injection rule

### Requirement: For-loop element typing
For a `for x in xs` head, the type checker SHALL require a known iterable type to be a non-optional array and SHALL bind `x` to the array element type within the loop body.

#### Scenario: Loop variable receives element type
- **WHEN** `users` has type `[User]` and the program evaluates `for u in users { u.id }`
- **THEN** the checker treats `u` as `User` and validates the `id` field access

#### Scenario: Non-array iterable
- **WHEN** the iterable expression has known type `int`
- **THEN** the checker reports that `for` requires an array

#### Scenario: Optional array iterable
- **WHEN** the iterable expression has known type `[User]?`
- **THEN** the checker reports that an optional array cannot be iterated without explicit handling

#### Scenario: Unknown iterable
- **WHEN** the iterable expression has no known type
- **THEN** the checker preserves existing behavior by leaving the loop variable type unknown

### Requirement: Array type names follow module resolution
The module loader SHALL resolve named leaf types inside array types using the same local, imported, and canonical-name rules as standalone named types.

#### Scenario: Imported array element type
- **WHEN** a module imports `User` and declares a field as `[User]`
- **THEN** the loaded program contains an array element type referring to the canonical imported `User`

### Requirement: Canonical program uses a declared array type
The canonical program SHALL declare the in-memory user collection as `[User]` and SHALL continue to pass static checking and execution tests.

#### Scenario: Canonical loop is statically typed
- **WHEN** the canonical program is checked
- **THEN** `self.users` has type `[User]`, the loop variable has type `User`, and the program produces no type diagnostics

#### Scenario: Canonical behavior is unchanged
- **WHEN** the canonical main program and test are executed
- **THEN** their existing successful results remain unchanged
