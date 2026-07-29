# runtime-diagnostic-spans

## Purpose

Failures that can only surface while a program runs identify the innermost
source expression that observed them and are rendered through the same
source-aware diagnostic pipeline as pre-execution failures, so a runtime error
shows the offending expression rather than a bare message.

## Requirements

### Requirement: Runtime failures identify the innermost failing expression
The evaluator SHALL attach the source span of the innermost evaluated expression that produces a runtime failure. Once a runtime diagnostic has a span, propagation through enclosing expressions, blocks, function calls, and callers MUST preserve that span.

#### Scenario: Direct expression failure
- **WHEN** an evaluated expression fails directly, such as an `assert` whose condition is false
- **THEN** the runtime diagnostic points to that failing expression

#### Scenario: Nested expression failure
- **WHEN** an inner expression fails while an enclosing expression is being evaluated
- **THEN** the runtime diagnostic points to the inner expression rather than the enclosing expression

#### Scenario: Failure in a called function
- **WHEN** a function body fails after being invoked from another expression
- **THEN** the runtime diagnostic points to the failing expression in the callee rather than the call site

#### Scenario: Failure in another module
- **WHEN** the failing expression belongs to a loaded module other than the entry module
- **THEN** the diagnostic span retains that module's source identity and byte range

### Requirement: Runtime failures use the shared diagnostic pipeline
An evaluation failure associated with a source expression SHALL be represented as a `Diag` and the CLI SHALL render it through the existing source-aware diagnostic renderer using the loaded source table. Runtime error message text SHALL remain available as the diagnostic message.

#### Scenario: Main program fails at runtime
- **WHEN** the entry function encounters a runtime failure
- **THEN** the CLI reports that main failed and renders the diagnostic with the failing source excerpt

#### Scenario: Test fails at runtime
- **WHEN** a test body or a function called by that test encounters a runtime failure
- **THEN** the CLI identifies the failing test, renders the diagnostic with the failing source excerpt, continues with the remaining tests, and includes the failure in the final test count

#### Scenario: Successful execution
- **WHEN** a main program or test completes without a runtime failure
- **THEN** its existing successful value or test result output is unchanged

### Requirement: Non-error evaluation flow remains distinct
The evaluator MUST keep language control flow separate from runtime diagnostics. In particular, a `return` caught at its function or test boundary SHALL produce the returned value and SHALL NOT be rendered as an error.

#### Scenario: Return exits a function
- **WHEN** a function evaluates a `return` expression
- **THEN** invocation yields the returned value without producing a runtime diagnostic

#### Scenario: Evaluator guard has no source expression
- **WHEN** the evaluator is asked to invoke an unknown entry name before any source expression is evaluated
- **THEN** it may return an unlocated diagnostic whose message still describes the failure
