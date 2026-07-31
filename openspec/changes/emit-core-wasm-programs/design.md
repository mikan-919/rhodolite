## Context

See `proposal.md` for motivation. The compiler pipeline is currently

```text
load AST → check/lower typed HIR → analyze requirements → interpret HIR
                                      └→ ambient_abi::Plan (not executed)
```

The module loader already distinguishes module identity from local import
names, supports selected imports and aliases, loads only dependency-reachable
files, and permits cyclic `use`. It intentionally reserved `pub use` without
implementing it. Typed HIR gives every expression a concrete result
classification and every call a resolved identity.

`ambient_abi::Plan` deterministically interns callable instances by body and
provider selection, records planned direct-call targets, and interns minimal
ambient-record layouts. Its current convenience entry point always roots
`main` and every test together. The interpreter does not consume the plan.

The first emitter must preserve those boundaries while adding a host-neutral
Core Wasm module. It cannot yet represent data values or the provider handles
stored in non-empty ambient records. Component Model, WIT, WASI, JavaScript,
and any framework lifecycle are outside the compiler contract.

## Goals / Non-Goals

**Goals:**

- Add public re-exports without changing declaration identity or ordinary
  import behavior.
- Make the set of planning roots explicit and reusable for production,
  existing plan tests, and a future compiled-test mode.
- Consume specialization instances and planned call targets in the first Wasm
  emitter, even while only empty runtime ambient layouts are executable.
- Generate a valid, deterministic, self-describing Core Wasm binary directly
  from checked HIR and the plan.
- Keep ABI wrappers shallow and separate from internal specialized functions.
- Make interpreter integer behavior match the Wasm execution meaning before
  differential testing begins.

**Non-Goals:**

- Emitting a WebAssembly Component, WIT, WASI imports, JavaScript or Rust
  bindings, or a host framework.
- Compiling strings, optionals, structs, enums, arrays, matching, methods,
  traits, slots, `with`, or non-empty ambient records.
- Defining linear-memory allocation, WasmGC layout, provider-handle layout,
  rich-type ownership, detailed error transport, source maps, optimization,
  or compiled test execution.
- Making all public declarations host-callable or recursively exporting a
  public module namespace.

## Decisions

### 1. Core Wasm is the stable compiler artifact; adapters remain downstream

The compiler emits a Core Wasm module containing ordinary function exports and
one Rhodolite metadata custom section. The module requires no imports and has
no Wasm start function. Instantiation is therefore inert; a framework chooses
when to call `__rhodolite_main` or another export.

Core Wasm was selected because it is the portable execution substrate shared
by browsers and standalone runtimes. A Component binary was rejected as the
normative v0 artifact because Rhodolite would inherit the Component Model,
Canonical ABI, and async evolution before its own data and async semantics are
settled. A JavaScript backend was rejected as the primary backend because it
would make JavaScript runtime semantics part of generated execution. Direct C
or native Cranelift emission remains possible later but is no longer the next
roadmap target.

WIT may eventually be generated from Rhodolite interface metadata, and a
framework may wrap the Core module into a Component. Neither representation is
normative for ABI v0.

### 2. `pub use` extends the existing import model rather than adding an export declaration

`UseDecl` gains a public flag while retaining its existing path, optional
alias, optional selected-member list, and span. Parsing accepts `pub use` only
where `use` is already accepted. Both forms load the same dependency and
introduce the same local binding. Only the public form also registers the
binding in the module's public-member table.

The loader resolves public bindings to the same canonical declaration or
module identities used by ordinary imports. Aliases change only the local and
public spelling. Public tables participate in member selection, so a selected
re-export can be selected again downstream. Resolution occurs after reachable
modules and their declared members are known; cyclic re-export dependencies
are iterated to a fixed point. Any unresolved selected member or collision
after convergence is diagnosed at its declaration span.

Every source declaration remains importable under the current visibility
model. This change adds explicit re-export, not declaration privacy. A
module-form `pub use users` or selected child-module re-export adds a namespace
member but does not recursively flatten that namespace.

The loaded program carries the entry module's explicitly selected public
bindings separately from its flattened AST items. Type checking resolves these
bindings to HIR identities. A general public-item identity preserves public
types and modules for language-level use, while a sorted
`(public_name, CallableId, span)` view supplies host function candidates.
Only functions introduced by an entry-module member-selection declaration
enter that view; module-form imports do not.

Adding a separate `export` declaration was rejected because module clients and
hosts would observe two public surfaces. Treating every loaded declaration as
public ABI was rejected because adding an internal dependency function would
silently change the host contract.

### 3. Planning accepts an explicit root set

Refactor the planner core to accept an ordered sequence of root requests. Each
request contains a `BodyId` and begins from an explicit provider context. A
small compatibility helper continues to produce the current `main + all
tests`, empty-context plan used by existing snapshots.

Production planning passes:

1. the entry `main` body;
2. each distinct explicitly exported `CallableId`, sorted by public export
   name;
3. an empty provider context for every root.

Several aliases may refer to the same callable. The planner interns one
specialization instance while the ABI layer creates one wrapper per public
name. The production root result retains the mapping from entry/export name to
its planned `InstanceId`.

Requirement diagnostics are generalized to accept named roots rather than
recognizing only `main` and tests by string convention. Each public function
must close its requirements from an empty context. `main` state is never
inherited by a later exported call.

Tests are omitted from production roots but remain supported by the planner's
existing test helper and snapshots. Deleting test planning was rejected
because it would discard completed work and obstruct a later compiled-test
artifact.

### 4. Reachability and backend support are separate checks

Loading and total static type checking continue to cover the whole loaded
program. Requirement analysis continues to compute its whole-program fixed
point. After production roots have passed unsatisfied-requirement checks, the
specialization plan defines backend reachability.

A Wasm support pass walks only planned instances and their reachable
expressions. It accepts:

- `unit`, `bool`, and `int` values;
- scalar literals, locals, bindings, and local assignment;
- negation, addition, subtraction, multiplication, division, and equality;
- blocks, `if`, `while`, direct free-function calls, `return`, and `assert`.

It rejects any reached unsupported type, expression, call form, method,
provision, or non-empty runtime record at the narrowest available HIR span.
The diagnostic names the construct and says it is unsupported by the current
Wasm target, rather than presenting it as a language type error.

Checking every loaded body for backend support was rejected because it defeats
demand-driven compilation and prevents a project from compiling a scalar
entry while retaining unused richer declarations. Silently skipping a reached
construct was rejected because it could produce a valid module with incorrect
behavior.

### 5. The emitter consumes `Plan` identities and call-site decisions

A Wasm backend module receives:

- typed HIR;
- the production specialization plan;
- the name-to-root-instance export mapping;
- validated ABI signatures.

It does not perform name resolution, overload selection, trait implementation
lookup, or requirement inference. One internal Wasm function is assigned per
reachable `InstanceId` in deterministic plan order. Planned direct calls map
their target instance to its assigned function index.

This first slice accepts only instances whose runtime ambient-record layout is
empty. It nevertheless establishes the final code-generation shape: functions
are generated from instances, not raw declarations. A later data/runtime
change can add the planned hidden record parameter and projections without
replacing function identity or call-target selection.

Supporting value providers immediately was rejected because provider handles
depend on the still-undecided representation of structs, object identity,
mutation, and memory management. Ignoring `Plan` until that phase was also
rejected because it would make the initial emitter use a second, temporary
function and call graph.

### 6. Stack lowering erases unit and preserves typed HIR control flow

Rhodolite `int` lowers to `i64`; `bool` lowers internally to canonical `i32`;
`unit` has no runtime value. Locals are allocated only for value-carrying
`bool` and `int` HIR locals. Binding or assigning unit evaluates any operand
effects without allocating a Wasm local.

Non-final value-producing expressions in a sequence are explicitly dropped.
Typed `if` expressions emit matching result stacks; unit branches leave no
value. `while` lowers to structured `block`/`loop` control flow. Direct
Rhodolite `return` branches to a function-level exit shape so later
expressions are not evaluated. A failed assertion emits `unreachable`.

Integer addition, subtraction, multiplication, and negation use the naturally
wrapping `i64` operations. Signed `i64.div_s` provides truncation toward zero
and traps for both specified failure cases. The interpreter changes to
`wrapping_add`, `wrapping_sub`, `wrapping_mul`, `wrapping_neg`, and checked
signed division so debug and release builds agree.

### 7. ABI wrappers isolate host validation from specialized bodies

Internal specialized functions use lowered scalar signatures but are not
exported. The emitter creates:

- one `__rhodolite_main` wrapper for the selected entry instance;
- one wrapper for each sorted public export name.

`main` must have no source parameters. Public parameters may be only `bool` or
`int`; public results and the entry result may be `unit`, `bool`, or `int`.
Unit parameters are rejected because erasing them would make source and host
arity disagree.

For each public Boolean parameter, the wrapper checks that the incoming `i32`
is zero or one and traps otherwise. It then directly calls the planned
instance. Internal Boolean producers are canonical by construction, and
Boolean wrappers additionally normalize their result to zero or one. Integer
bits pass unchanged through `i64`. Unit results produce no Wasm result.

`__rhodolite_main` is reserved at the ABI layer. A public alias with that name
is diagnosed before emission. The wrapper is an explicit export rather than a
Wasm start function because `main` may return a value and framework lifecycle
is not compiler policy.

Encoding status returns or error buffers was rejected because it would alter
every signature and prematurely require memory ownership. ABI v0 exposes
runtime failure as a Wasm trap.

### 8. `rhodolite.abi` is canonical embedded JSON

The module contains exactly one `rhodolite.abi` custom section. Its UTF-8
payload has this schema:

```json
{
  "version": 0,
  "entry": {
    "name": "__rhodolite_main",
    "params": [],
    "result": "bool"
  },
  "exports": [
    {
      "name": "find_user",
      "params": ["int"],
      "result": "bool"
    }
  ]
}
```

The actual encoding is compact: no BOM, insignificant whitespace, or trailing
newline. Object keys appear in the order shown: top-level `version`, `entry`,
`exports`; function `name`, `params`, `result`. Public export records are
sorted by export-name UTF-8 bytes; parameter types retain source order. Scalar
type strings are exactly `unit`, `bool`, and `int`, though `unit` can occur
only as a result in ABI v0. The entry record retains an empty `params` array so
the schema can be parsed uniformly.

JSON was selected over a separate sidecar because the `.wasm` must remain a
self-contained distribution unit. It was selected over a custom binary,
CBOR, or MessagePack encoding because the metadata is tiny and framework
authors should be able to inspect it without Rhodolite-specific binary
tooling. This JSON is Rhodolite's contract; it is not a custom source IDL and
users do not author it.

### 9. Binary emission is direct, validated, deterministic, and published atomically

Use a focused Core Wasm encoder to construct the binary in memory and a
separate validator to validate the completed bytes before publication. Section
order, function-index assignment, type interning, local ordering, export
ordering, and metadata ordering are all derived from stable IDs or explicitly
sorted names. Hash iteration order, filesystem enumeration order, and pointer
identity cannot affect output.

Tests use an in-process Core Wasm engine as a development dependency to invoke
the entry and public wrappers, including raw invalid Boolean inputs. The engine
is a test oracle for portability, not part of the emitted ABI or production
CLI. Focused encoder/validator libraries are preferred over hand-writing LEB128
and binary section rules. A second test hashes or directly compares two
emissions from the same input.

The build encodes and validates completely before touching the destination.
It creates the destination directory, writes a same-directory temporary file,
and renames it into place only after the write completes. On any earlier
failure, an existing output remains unchanged and no partial requested output
is published.

### 10. CLI parsing becomes explicit without changing interpreter output

Replace the current single positional lookup with a small explicit command
model:

```text
rhodolite [<entry.rd>]
rhodolite build <entry.rd> --target wasm [-o <output.wasm>]
```

No argument continues to interpret `examples/canonical.rd`. The build command
uses the existing loader, type checker, requirement analyzer, and diagnostic
renderer, but it does not print the interpreter's declaration inventory,
requirement report, or execution result on success. It reports the final output
path. Unknown targets, missing option values, duplicate output options, and
unexpected arguments fail with concise usage diagnostics.

The default output is resolved from the invocation directory as
`target/wasm/<entry-file-stem>.wasm`, not beside a dependency or relative to
the entry file's parent. An explicit `-o` is interpreted relative to the
invocation directory.

### 11. Architecture records are updated from C to Wasm

Add an ADR recording Core Wasm plus Rhodolite ABI v0, public re-exports as the
host surface, and Component/WIT as adapters. Update the compiler roadmap so
the next stages are:

```text
emit-core-wasm-programs
compile-wasm-data-values
compile-wasm-traits-and-ambient
add-differential-execution
```

ADR-0008's C snippets remain explanatory pseudocode for the logical ambient
record contract, but references claiming C is the selected backend are updated
to backend-neutral or Wasm wording. Historical archived change artifacts are
not rewritten.

## Risks / Trade-offs

- [The first artifact cannot compile the canonical data-rich example] →
  Include executable scalar fixtures and keep the next data-value change
  explicit in the roadmap.
- [Public re-export cycles complicate module resolution] → Resolve public
  identities after declaration collection with a deterministic fixed point and
  diagnose unresolved selections after convergence.
- [Unit erasure can produce invalid Wasm stack shapes] → Drive lowering from
  HIR result classifications and test every control-flow form with validation.
- [ABI wrappers and internal functions can accidentally diverge] → Derive both
  from one checked source signature and invoke wrappers through an independent
  engine in tests.
- [A custom ABI creates ecosystem work] → Keep v0 deliberately tiny,
  self-describing, and mechanically convertible to future WIT or
  host-framework bindings.
- [Trap-only failures lack Rhodolite source diagnostics] → Preserve the
  interpreter's diagnostics and defer structured compiled errors until memory
  and rich ABI values exist.
- [New encoder, validator, JSON, and test-engine dependencies increase build
  cost] → Use narrowly scoped libraries, keep the runtime engine
  development-only, and avoid a production JIT dependency.

## Migration Plan

1. Land explicit integer semantics in the interpreter with boundary regression
   tests; existing in-range program behavior remains unchanged.
2. Land `pub use` parsing and module resolution while keeping plain `use`
   behavior and all existing module tests unchanged.
3. Generalize root selection and prove the legacy combined plan snapshot is
   byte-for-byte unchanged.
4. Add Wasm support checking, emission, ABI metadata, validation, and
   independent execution tests behind the new build command.
5. Update the ADR, overview, and roadmap only after the vertical slice passes
   formatting, unit, CLI, validation, determinism, and execution tests.

Each step is committed as a stable snapshot. Rollback removes the new build
command and emitter without affecting positional interpreter execution; the
public re-export and integer-semantics snapshots are independently valid
language improvements.
