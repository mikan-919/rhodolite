## MODIFIED Requirements

### Requirement: Reachable owned data programs lower to Core Wasm
The Core Wasm backend SHALL lower reachable ownership-safe uses of `str`, owned structs, payload enums, optionals, arrays, declared indirect values, field access and assignment, ownership modifiers, `clone()`, `??`, `match`, and `for`. It SHALL preserve the runtime behavior specified by their existing language capabilities and by `wasm-owned-data-values`, including when those values cross method, trait implementation, and ambient slot calls supported by `wasm-traits-and-ambient`.

#### Scenario: Owned data program executes
- **WHEN** a production root reaches strings, an owned struct, a payload enum, an optional, and an array using supported operations
- **THEN** the generated module validates and produces the same result as the reference interpreter

#### Scenario: Canonical data subset builds
- **WHEN** the canonical program reaches owned-data expressions through methods and ambient slot calls
- **THEN** those expressions are lowered without an unsupported Wasm-target diagnostic

### Requirement: Emission follows the specialization plan
Wasm functions for Rhodolite bodies SHALL be emitted from deterministic specialization instances and planned direct-call targets rather than by performing a second name or implementation lookup. Every instance SHALL use the provider combination in its specialization key. An instance with value-level ambient requirements SHALL receive only the provider handles in its planned runtime-record layout; an instance with an empty layout SHALL receive no hidden ambient values.

#### Scenario: Shared reachable instance is emitted once
- **WHEN** more than one production root reaches the same specialization instance
- **THEN** the module contains one implementation of that instance and all callers target it directly

#### Scenario: Runtime provider record is required
- **WHEN** reachable code requires a specialization instance with a non-empty ambient-record layout
- **THEN** the instance is emitted with the planned provider handles and its callers pass the corresponding projected handles

#### Scenario: Provider combinations differ
- **WHEN** the same body is reachable with two different provider implementation combinations
- **THEN** each combination has one deterministic specialization instance and calls do not dispatch between them at runtime

### Requirement: Unsupported checks are reachability-sensitive
All loaded code SHALL still pass the ordinary whole-program static and ownership checks. Wasm-target support SHALL then be checked only for specialization instances reachable from production roots. A reachable expression, value type, provider ownership form, runtime record, or public boundary outside the supported scalar, owned-data, and trait-and-ambient subsets SHALL cause a source-positioned diagnostic naming the unsupported construct. Unsupported unreachable code SHALL not block production emission.

#### Scenario: Unsupported declaration is unreachable
- **WHEN** a loaded but unreachable function uses aggregate-stored borrows or another feature outside the supported Wasm subset
- **THEN** the declaration remains statically checked but does not prevent a supported production build

#### Scenario: Unsupported construct is reachable
- **WHEN** a production root reaches a construct outside the supported scalar, owned-data, and trait-and-ambient subsets
- **THEN** the build fails before publishing the artifact and points to the reached construct

#### Scenario: Trait and ambient construct is reachable
- **WHEN** a production root reaches an ownership-safe method, trait implementation, slot call, or `with` expression
- **THEN** the construct is checked and lowered rather than rejected merely for using trait or ambient behavior
