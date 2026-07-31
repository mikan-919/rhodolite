## Purpose

Defines explicit public re-exports so a module can present a stable, named API
without making ordinary imports transitive or exposing an entire dependency by
accident.

## ADDED Requirements

### Requirement: Modules explicitly re-export imported names
A top-level `pub use` declaration SHALL perform the same loading, member
selection, aliasing, and local name introduction as the corresponding `use`
declaration and SHALL additionally add the introduced name to the declaring
module's public member namespace. A plain `use` SHALL remain local to its
declaring module. `pub use` SHALL obey the existing restriction that use
declarations form a top-level prefix.

#### Scenario: Selected declaration is re-exported
- **WHEN** a module declares `pub use users::{find}`
- **THEN** `find` is available both as a local name in that module and as a public member selectable from that module

#### Scenario: Alias is the public name
- **WHEN** a module declares `pub use users::{find as find_user}`
- **THEN** `find_user` is the local and public name and `find` is not re-exported by that declaration

#### Scenario: Plain use remains private to the importing module
- **WHEN** a module declares `use users::{find}` without `pub`
- **THEN** `find` is available locally but is not a selectable member re-exported by that module

#### Scenario: Pub use outside the use prefix is rejected
- **WHEN** `pub use` appears after a non-use top-level item or inside a body
- **THEN** parsing fails with a source-positioned diagnostic under the same placement rule as `use`

### Requirement: Re-exports support every importable member kind
`pub use` SHALL accept every declaration or child-module member that the
corresponding `use` form can select. Re-exporting a member SHALL preserve its
original declaration identity even when one or more aliases or intermediate
re-exports change its visible name.

#### Scenario: Type declaration is re-exported
- **WHEN** a module publicly selects a struct or enum from another module
- **THEN** downstream modules can select that type through the re-exporting module

#### Scenario: Function is re-exported through an intermediate module
- **WHEN** an intermediate module publicly selects a function and the entry module publicly selects that intermediate public name
- **THEN** the entry module's public name resolves to the original function declaration

#### Scenario: Module namespace is re-exported
- **WHEN** a module uses a module-form `pub use` declaration
- **THEN** downstream modules can select that namespace without its descendant functions being converted into individually selected public names

### Requirement: Public names remain explicit and collision-free
Publicly introduced names SHALL occupy the declaring module's existing
single member namespace. A public name that conflicts with a local declaration,
ordinary import, or another public import SHALL be rejected rather than
overwritten. Cyclic module loading SHALL remain supported; public re-export
resolution that cannot identify an original member after resolving the loaded
module graph SHALL fail with a source-positioned missing-member diagnostic.

#### Scenario: Two public imports collide
- **WHEN** two `pub use` declarations introduce the same local public name
- **THEN** loading fails and identifies the conflicting public name

#### Scenario: Alias resolves a collision
- **WHEN** conflicting declarations are publicly imported under distinct aliases
- **THEN** both aliases are accepted as distinct public members

#### Scenario: Re-export cycle has no originating member
- **WHEN** cyclic public selections only refer back to one another and no loaded module declares the selected member
- **THEN** loading fails instead of manufacturing a public declaration

### Requirement: Entry function exports require explicit member selection
The entry module's host-callable function surface SHALL contain only functions
introduced by an explicit member-selecting `pub use path::{...}` declaration.
A module-form public re-export SHALL expose a language-level namespace but SHALL
NOT recursively add descendant functions to the host-callable surface.

#### Scenario: Selected function enters the host surface
- **WHEN** the entry module declares `pub use users::{find as find_user}`
- **THEN** the host-callable surface contains the original function under the name `find_user`

#### Scenario: Public namespace does not recursively expose functions
- **WHEN** the entry module declares only `pub use users`
- **THEN** no function beneath `users` enters the host-callable surface solely because of that declaration

#### Scenario: Selected non-function remains a language export only
- **WHEN** the entry module explicitly publicly selects a struct or enum
- **THEN** the declaration is part of the language-level public namespace but creates no host-callable function
