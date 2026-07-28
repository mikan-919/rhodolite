# fieldless-enums

## Purpose

Declare a finite set of named, payload-free values as a single type, and carry
that membership from declaration through name resolution into evaluation. This
is the first field-*value* type fact the checker uses: `Gold` types as `Rank`.

## Requirements

### Requirement: Fieldless enum declarations define a finite type
The language SHALL accept an enum declaration containing zero or more distinct, payload-free variants and SHALL associate every variant with the declared enum type.

#### Scenario: Enum with variants
- **WHEN** a program declares `enum Rank { Bronze Gold }`
- **THEN** `Rank` is a declared enum type and `Bronze` and `Gold` are its variants

#### Scenario: Empty enum
- **WHEN** a program declares `enum Never {}`
- **THEN** `Never` is a declared enum type with no values

#### Scenario: Duplicate variant
- **WHEN** an enum declaration names the same variant more than once
- **THEN** checking fails with a deterministic diagnostic identifying the enum and duplicate variant

### Requirement: Variants participate in ordinary name resolution
Each enum variant SHALL remain a value declaration in its module's existing declaration namespace and SHALL follow existing canonical-name, import, collision, and lexical-shadowing rules. The language SHALL additionally resolve `Enum::Variant` as a qualified variant value when `Enum` names a declared enum and `Variant` belongs to it.

#### Scenario: Bare local variant
- **WHEN** code in the declaring module refers to an unshadowed `Gold`
- **THEN** the reference resolves to that module's `Gold` variant

#### Scenario: Imported variant
- **WHEN** another module imports `Gold` through the existing member-import syntax
- **THEN** an unshadowed `Gold` reference resolves to the imported variant's canonical name

#### Scenario: Qualified local variant
- **WHEN** code refers to `Rank::Gold` and `Rank` is a visible enum containing `Gold`
- **THEN** the path resolves to the same variant declaration as the corresponding unshadowed bare `Gold`

#### Scenario: Qualified imported enum
- **WHEN** another module imports or qualifies enum `Rank` and refers to `Rank::Gold`
- **THEN** the enum name follows existing canonical-name and import rules and the path resolves to `Rank`'s `Gold` variant

#### Scenario: Unknown qualified variant
- **WHEN** code refers to `Rank::Silver` but `Rank` has no `Silver` variant
- **THEN** loading or checking fails with a deterministic diagnostic identifying `Rank::Silver`

#### Scenario: Non-enum qualifier
- **WHEN** a value path uses `Thing::Value` but `Thing` is not a declared enum
- **THEN** the path does not become a variant value and existing path or call diagnostics apply

#### Scenario: Declaration collision
- **WHEN** a variant and another declaration expose the same bare name in one module
- **THEN** loading fails through the existing duplicate-declaration diagnostic

#### Scenario: Local shadows bare variant
- **WHEN** a parameter, `let` binding, loop binding, `self`, or ambient binding shadows a bare variant name
- **THEN** bare references in that lexical scope resolve to the local binding rather than the variant

#### Scenario: Local does not shadow qualified variant
- **WHEN** a local binding is named `Gold` and code refers to `Rank::Gold`
- **THEN** the qualified path continues to resolve through the enum declaration

### Requirement: Variants evaluate as enum values
An unshadowed bare variant reference or a resolved qualified variant path SHALL evaluate to an immutable enum value that retains both its enum identity and variant identity.

#### Scenario: Evaluate a bare variant
- **WHEN** `Gold` resolves to the `Gold` variant of `Rank`
- **THEN** evaluation produces the `Rank.Gold` enum value

#### Scenario: Evaluate a qualified variant
- **WHEN** `Rank::Gold` resolves to the `Gold` variant of `Rank`
- **THEN** evaluation produces the same `Rank.Gold` enum value as the bare reference

#### Scenario: Equal bare and qualified variants
- **WHEN** one value is produced from bare `Gold` and another from `Rank::Gold` for the same declaration
- **THEN** equality comparison evaluates to true

#### Scenario: Different variants
- **WHEN** two values belong to different variant declarations
- **THEN** equality comparison evaluates to false

#### Scenario: Enum is not a struct
- **WHEN** code attempts field access on an enum value
- **THEN** evaluation fails instead of treating the enum as a zero-field struct

### Requirement: Known enum values are checked at enum-typed struct fields
The checker SHALL reject a struct construction or field assignment when the destination is a known, non-optional enum type and the source is known to be a variant of a different enum. It SHALL accept a variant of the expected enum.

#### Scenario: Matching variant in struct construction
- **WHEN** field `rank` is declared as `Rank` and a struct literal gives it `Gold` from `Rank`
- **THEN** checking produces no enum-type diagnostic

#### Scenario: Mismatched variant in struct construction
- **WHEN** field `rank` is declared as `Rank` and a struct literal gives it a variant from another enum
- **THEN** checking fails with a diagnostic identifying the expected and actual enum types

#### Scenario: Matching variant in field assignment
- **WHEN** a receiver is known to have a `rank: Rank` field and code assigns `Gold` from `Rank`
- **THEN** checking produces no enum-type diagnostic

#### Scenario: Mismatched variant in field assignment
- **WHEN** a receiver is known to have a `rank: Rank` field and code assigns a variant from another enum
- **THEN** checking fails with a diagnostic identifying the expected and actual enum types

#### Scenario: Source type is not yet known
- **WHEN** an enum-typed field receives an expression whose type is outside this change's inference boundary
- **THEN** this capability emits no enum-type diagnostic for that expression

### Requirement: The canonical program uses a declared Rank enum
The canonical program SHALL declare `Rank` as a fieldless enum and SHALL use its variants without zero-field struct stand-ins.

#### Scenario: Canonical rank values
- **WHEN** the canonical program constructs a bronze user and later promotes that user to gold
- **THEN** `Bronze` and `Gold` both type as `Rank`, the program passes checking, and the existing promotion test succeeds
