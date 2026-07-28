## MODIFIED Requirements

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
