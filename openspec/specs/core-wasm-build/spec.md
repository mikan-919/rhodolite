# core-wasm-build

## Purpose

Adds a deterministic Core WebAssembly build path for the scalar subset while
retaining the interpreter as Rhodolite's reference execution implementation.

## Requirements

### Requirement: The CLI builds Core Wasm explicitly
The CLI SHALL accept
`rhodolite build <entry.rd> --target wasm [-o <output.wasm>]`. It SHALL keep
the existing positional source invocation as interpreter execution. With no
`-o`, a successful build SHALL write
`target/wasm/<entry-file-stem>.wasm` relative to the invocation directory and
create missing output directories. With `-o`, it SHALL write only the selected
output path.

#### Scenario: Default Wasm build succeeds
- **WHEN** `rhodolite build app.rd --target wasm` is invoked for a supported program
- **THEN** a Core Wasm module is written to `target/wasm/app.wasm`

#### Scenario: Explicit output path is honored
- **WHEN** a supported program is built with `-o dist/service.wasm`
- **THEN** the module is written to `dist/service.wasm` and not to the default path

#### Scenario: Positional invocation remains interpreted
- **WHEN** `rhodolite app.rd` is invoked
- **THEN** the existing interpreter path runs and no Wasm artifact is produced

#### Scenario: Failed build does not publish an artifact
- **WHEN** loading, checking, requirement analysis, lowering, validation, or output writing fails
- **THEN** the command exits unsuccessfully and does not replace the requested output with a partial module

### Requirement: Production builds use explicit production roots
A production Wasm build SHALL plan and emit the entry `main` function plus each
function explicitly selected into the entry module's host-callable surface.
Every production root SHALL begin with an empty ambient provider context.
Tests SHALL remain available to internal test planning but SHALL NOT become
production roots or enter the production artifact solely because they are
declared.

#### Scenario: Main and public functions are roots
- **WHEN** the entry module publicly selects two functions
- **THEN** `main` and both selected functions are independently planned from empty provider contexts

#### Scenario: Public ambient requirement is not closed
- **WHEN** a public function has an ambient requirement that remains unsatisfied at its empty root
- **THEN** the build fails before Wasm emission with the existing requirement path information

#### Scenario: Test-only unsupported code is excluded
- **WHEN** a test uses a feature outside the scalar Wasm subset but no production root reaches that code
- **THEN** the production Wasm build can succeed and the test is not emitted

### Requirement: Scalar reachable programs lower to Core Wasm
The initial Wasm backend SHALL lower reachable uses of `unit`, `bool`, and
`int`; scalar literals and locals; local binding and assignment; arithmetic and
equality; blocks; `if`; `while`; direct free-function calls; `return`; and
`assert`. It SHALL preserve expression evaluation order, short-circuiting
control flow, local mutation, direct-call results, and the integer semantics
defined by `integer-runtime-semantics`. A false assertion SHALL trap.

#### Scenario: Scalar control-flow program executes
- **WHEN** a program uses scalar locals, assignment, a loop, a conditional, direct calls, and return
- **THEN** its generated module executes to the same successful result as the interpreter

#### Scenario: False assertion traps
- **WHEN** reachable generated code evaluates `assert false`
- **THEN** the Wasm invocation traps

#### Scenario: Direct return exits its function
- **WHEN** a reachable branch executes a value-carrying `return`
- **THEN** the generated function returns that value without evaluating later expressions in its body

### Requirement: Emission follows the specialization plan
Wasm functions for Rhodolite bodies SHALL be emitted from deterministic
specialization instances and planned direct-call targets rather than by
performing a second name or implementation lookup. Instances whose runtime
ambient-record layout is empty MAY be emitted. Reaching an instance that needs
a non-empty runtime ambient record SHALL fail as unsupported in this version.

#### Scenario: Shared reachable instance is emitted once
- **WHEN** more than one production root reaches the same specialization instance
- **THEN** the module contains one implementation of that instance and all callers target it directly

#### Scenario: Runtime provider record is required
- **WHEN** reachable code requires a specialization instance with a non-empty ambient-record layout
- **THEN** the build fails with a source-positioned unsupported-feature diagnostic

### Requirement: Unsupported checks are reachability-sensitive
All loaded code SHALL still pass the ordinary whole-program static checks.
Wasm-subset support SHALL then be checked only for specialization instances
reachable from production roots. A reachable expression, call form, value type,
or runtime record outside the supported subset SHALL cause a source-positioned
diagnostic naming the unsupported construct. Unsupported unreachable code SHALL
not block production emission.

#### Scenario: Unsupported declaration is unreachable
- **WHEN** a loaded but unreachable function uses strings, structs, enums, optionals, arrays, matching, methods, traits, ambient calls, or `with`
- **THEN** the declaration remains statically checked but does not prevent a supported production build

#### Scenario: Unsupported construct is reachable
- **WHEN** a production root reaches one of those unsupported constructs
- **THEN** the build fails before publishing the artifact and points to the reached construct

### Requirement: Wasm artifacts are valid and deterministic
A successful build SHALL produce a valid Core WebAssembly module with no
required host imports and no automatic Wasm start section. Its functions SHALL
run only when the host calls an export. Repeated builds of the same loaded
program, options, and compiler version SHALL produce byte-identical modules.

#### Scenario: Module validates and runs in a Core Wasm engine
- **WHEN** a supported build succeeds
- **THEN** an independent Core Wasm validator accepts the artifact and a Core Wasm engine can invoke its exported functions

#### Scenario: Instantiation does not run main
- **WHEN** a host instantiates the generated module without invoking an export
- **THEN** `main` is not executed

#### Scenario: Build is repeated
- **WHEN** the same supported input is built twice with the same compiler and options
- **THEN** the two module byte sequences are identical
