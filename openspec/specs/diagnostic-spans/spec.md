# diagnostic-spans

## Purpose

Every diagnostic produced before execution carries the byte range that caused it
and the file that range belongs to, so the CLI can show the offending source
rather than a bare message. Unsatisfied ambient requirements point at the use
site and mark each hop of their reachability path at the call that propagates
it, even when the path crosses modules.

## Requirements

### Requirement: Spans identify their source file
Every span produced by lexing SHALL carry an identifier for the source file it was read from, in addition to its byte range. Module loading SHALL assign each loaded source file a distinct identifier and SHALL retain that file's path and text for the lifetime of the loaded program. A span taken from any item or expression of a merged multi-module program SHALL resolve to exactly one loaded source file.

#### Scenario: Span from a single-file program
- **WHEN** a program consisting of one file is loaded
- **THEN** every span in its syntax tree resolves to that file's path and text

#### Scenario: Span from a merged multi-module program
- **WHEN** modules from several files are merged into one program
- **THEN** each span still resolves to the file it was lexed from, and byte ranges from different files do not collide

#### Scenario: Source text is retained
- **WHEN** loading succeeds
- **THEN** the loaded program exposes the path and full text of every module it read

### Requirement: Pre-execution diagnostics carry a position
Diagnostics produced by lexing, parsing, module loading, type checking, and requirement analysis SHALL each carry an optional primary span, an optional label for that span, an optional help note, and optional related diagnostics that may point at other files. A diagnostic SHALL omit its span only when no source position exists for it. The diagnostic message text, the conditions that produce each diagnostic, the order diagnostics are reported in, and the process exit code SHALL be unchanged by the addition of spans.

#### Scenario: Type diagnostic points at the offending expression
- **WHEN** type checking reports a mismatch for an expression
- **THEN** the diagnostic's primary span covers that expression

#### Scenario: Declaration diagnostic points at the declaration
- **WHEN** checking reports a problem with a declaration rather than an expression
- **THEN** the diagnostic's primary span covers the offending declaration

#### Scenario: Unresolvable module points at the use declaration
- **WHEN** a `use` declaration names a module that cannot be found
- **THEN** the diagnostic's primary span covers that `use` declaration

#### Scenario: Diagnostic without a source position
- **WHEN** loading fails before any source is read, such as an entry path that is not a `.rd` file or that cannot be read
- **THEN** the diagnostic carries no span and is still reported

#### Scenario: Existing messages are preserved
- **WHEN** a program that previously produced a given diagnostic is checked
- **THEN** the same message text is produced for the same condition, in the same order

### Requirement: The CLI renders diagnostics against their source
When reporting pre-execution diagnostics, the CLI SHALL render each diagnostic with its message, and, when the diagnostic has a span, the file path, the line and column of the span, an excerpt of the source at that position, and an indication of the span's extent. Related diagnostics SHALL be rendered against their own source file. Runtime errors from evaluation SHALL keep their current rendering.

#### Scenario: Diagnostic with a span
- **WHEN** the CLI reports a diagnostic that has a span
- **THEN** the output names the file, gives the position, and shows the source excerpt with the span indicated

#### Scenario: Diagnostic without a span
- **WHEN** the CLI reports a diagnostic that has no span
- **THEN** the output shows the message without a source excerpt

#### Scenario: Related diagnostic in another file
- **WHEN** a diagnostic's related entry points into a different module than the primary span
- **THEN** that entry is rendered with its own file's path and excerpt

#### Scenario: Runtime errors are unchanged
- **WHEN** evaluation fails after checking succeeded
- **THEN** the error is reported as it is today, without a source excerpt

### Requirement: Unsatisfied ambient requirements report the path by position
An unsatisfied ambient requirement SHALL be reported with a primary span at a use site that requires the slot, and SHALL carry one related entry per hop of its reachability path, each spanning the call that propagates the requirement. The existing single-line path rendering of function names SHALL remain available as the diagnostic's help note, and the inferred requirement listing SHALL be unchanged.

#### Scenario: Direct unsatisfied use
- **WHEN** an entry function itself uses a slot that is never provided
- **THEN** the diagnostic's primary span covers that use and there are no hops

#### Scenario: Transitive unsatisfied use
- **WHEN** a slot required several call levels below an entry is never provided
- **THEN** the primary span covers the use site and each intervening call contributes a related entry at its call span

#### Scenario: Path crosses modules
- **WHEN** the calls along a reachability path are declared in different modules
- **THEN** each hop is rendered against the file that declares it

#### Scenario: Path help note
- **WHEN** an unsatisfied requirement with a non-empty path is reported
- **THEN** the diagnostic's help note contains the existing arrow-separated chain of function names

#### Scenario: Requirement visualization is unchanged
- **WHEN** the CLI prints the inferred requirements of a program
- **THEN** the listing is identical to what it produced before spans were added
