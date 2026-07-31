## Context

See `proposal.md` for motivation. Rhodolite currently has a single
`typecheck::check_and_lower` boundary that returns typed, name-resolved HIR.
Requirement analysis, the HIR interpreter, ambient specialization, and Core
Wasm emission all consume that HIR. Compound interpreter values use shared
runtime identity, while HIR types carry only nominal shape and optionality; no
ownership, reference, scope, or access-mode fact survives lowering.

The compiler already loads and checks the whole module graph, assigns stable
in-program IDs, resolves every call, computes recursive fixed points for
requirements and specialization, and emits only after whole-program success.
Those constraints make lifetime elision possible, but the new pass must remain
deterministic and must not make later stages repeat name, type, or call
resolution. Existing Rhodolite blocks are not uniformly lexical scopes: a
second-class block deliberately lets ordinary locals flow into its surrounding
body, while function bodies, match arms, loop bindings, and provision bodies
already establish the language's actual scope boundaries. Ownership must follow
those boundaries rather than treating every pair of braces as a Rust block.

The current Wasm backend supports only `unit`, `bool`, and `int`. This change
must preserve that working vertical slice while producing the ownership facts
that the later `compile-wasm-owned-data-values` change will consume.

## Goals / Non-Goals

**Goals:**

- Establish a statically checked single-owner model with concise shared reads,
  explicit mutation, explicit call-site consumption, and explicit copying.
- Infer non-lexical borrow regions and return provenance over the complete
  loaded call graph without source lifetime parameters or runtime checks.
- Give downstream consumers one ownership-checked HIR boundary containing
  resolved access, provenance, scope, and destruction facts.
- Preserve deterministic diagnostics, existing module identity, ambient
  planning, interpreter behavior for migrated valid programs, and scalar Wasm
  output.
- Keep the internal model extensible to aggregate-stored borrows, explicit
  shared ownership, owned Wasm data layouts, and async state machines.

**Non-Goals:**

- Wasm storage layouts, a Wasm allocator, rich public ABI values, or compilation
  of any newly owned non-scalar value.
- Shared ownership, reference counting, tracing GC, weak references, cycles of
  runtime aliases, or interior-mutability containers.
- Source lifetime names, user-defined destruction, unwinding, raw pointers,
  manual free, or any `unsafe` construct.
- References stored inside structs, enums, optionals, or arrays.
- Partial moves, array-index theorem proving beyond simple known disjointness,
  borrowed public Wasm interfaces, async, closures, or separate compilation.

## Decisions

### 1. Split ownership semantics from Wasm data representation

The roadmap becomes:

```text
emit-core-wasm-programs
        ↓
introduce-ownership-and-borrowing
        ↓
compile-wasm-owned-data-values
        ↓
add-explicit-shared-ownership (only when concrete cases require it)
        ↓
compile-wasm-traits-and-ambient
```

The ownership change migrates parsing, checking, HIR, analysis, and the
interpreter before any memory layout is committed. Combining the work was
rejected because bugs in semantic ownership, borrow analysis, allocator code,
and Wasm lowering would be indistinguishable and no reference execution path
would exist for the new data meaning.

### 2. Surface syntax exposes capabilities, not lifetime variables

Types gain `&T` and `&mut T`; there is no lifetime grammar. `let` is immutable
and `let mut` grants mutation. Signatures use owned `T`, shared `&T`, mutable
`&mut T`, and receiver forms `self`, `&self`, and `&mut self`.

Access expressions gain modes `Shared`, `Mutable`, and `Move`. Source writes
`&place`, `&mut place`, or `move place` where a stored reference, mutable
capability, or ownership transfer must be visible. Calls auto-insert only shared
borrows. A bound non-Copy value entering an owned parameter requires `move`,
while a fresh temporary is already an owner and does not. Returns to owned
result types move automatically because the return boundary already states the
transfer.

Ownership prefixes are parsed as place/receiver modifiers, not ordinary
low-precedence unary operators. For a terminal method call,
`&mut account.user.rename(name)` qualifies `account.user` as the receiver;
`move value.finish()` qualifies `value`. Without a method call the qualifier
applies to the complete projected place, as in `&mut user.name` and
`move user.name`. Parentheses remain accepted but are not required.

The AST records:

- `TypeMode::{Owned, Shared, Mutable}` on every type;
- `mutable` on `let` declarations;
- `ReceiverMode::{Owned, Shared, Mutable}` rather than `has_self`;
- `AccessMode::{Shared, Mutable, Move}` plus an explicit access expression;
- `indirect` on each struct field and enum payload position.

Using an `own`, `borrow`, or lifetime-heavy syntax was rejected as less familiar
without adding information. Making every call-site shared borrow explicit was
rejected as noise; hiding `&mut`, `move`, or `clone` was rejected because those
operations change observable state, ownership, or cost.

### 3. HIR distinguishes value type from access mode

HIR `Type` gains `RefKind::{Shared, Mutable}` around an underlying owned value
shape rather than encoding references as nominal builtins. Optionality remains
on owned shapes; the checker rejects optional references and any reference
nested beneath array, struct-field, or enum-payload ownership in this version.
This representation deliberately supports those shapes later without treating
them as strings or new nominal declarations.

The ordinary type checker continues to resolve declarations, fields, calls,
expected types, and receiver signatures. It also resolves each syntactic
ownership qualifier and inserts required shared auto-borrows, but it does not
attempt loan lifetimes while the body is still being lowered.

A new ownership boundary returns:

```text
ownership::CheckedProgram {
    hir: hir::Program,
    plan: ownership::Plan,
}
```

`Plan` contains one deterministic body plan per callable/test, classified
accesses per `ExprId`, lexical `ScopeId`s, drop actions on control-flow edges,
and callable return-provenance summaries. Requirement analysis, evaluation,
ambient planning, and Wasm building accept `CheckedProgram` (or explicit
read-only views from it), so unchecked HIR cannot accidentally enter a normal
execution path. The plan references existing dense HIR IDs and never repeats
source-name lookup.

Mutating HIR in place with optional ownership fields was rejected because it
would make an unchecked `None` state representable downstream. Duplicating the
entire HIR into an owned-HIR tree was rejected because it would break the
existing ID-based planner and double the compiler's semantic representation.

### 4. Copy is deliberately small; clone is explicit and structural

`unit`, `bool`, `int`, and fieldless enums are Copy. `&T` is a copyable shared
capability; `&mut T` is not copyable and may only be reborrowed subject to its
current loan. Structs, payload enums, `str`, arrays, and optionals containing
non-Copy values are non-Copy by default. A `copy struct` opt-in is left for a
later capability rather than silently deriving potentially large copies.

`clone()` is compiler-known during this transition and produces an owned deep
clone of every cloneable component. Calling `clone()` on `&T` clones the
referent into owned `T`; it does not manufacture shared ownership. `&mut T` is
not cloneable. Once resource and user-defined trait facilities exist, this
builtin classification can become an ordinary `Clone` contract without
changing call sites.

Automatic structural derivation was selected because all current data values
are language-owned and have no resource hooks. Implicit clone on assignment or
call was rejected because it would hide allocation and input-size-dependent
work.

### 5. Ownership analysis operates on CFG places and loans

Each body is converted to a compact control-flow graph keyed back to `ExprId`.
Structured HIR remains the source representation; the CFG is analysis data,
not a second lowering consumed by the interpreter. Edges cover fallthrough,
conditionals, loop backedges, `return`, and diverging expressions.

A place is a local root plus projections:

```text
Place = Local(LocalId)
      + Field(FieldId)
      + OptionalPayload
      + EnumPayload(VariantId, position)
      + ArrayElement(KnownIndex | UnknownIndex)
```

Dataflow tracks initialization (`Uninitialized`, `Owned`, `Moved`), binding
mutability, and loans (`Shared` or `Mutable`) over places. A move of a projected
struct field consumes the whole struct root and schedules the other fields for
drop, so partial initialization states are unnecessary. Match mode applies to
the whole enum. At joins, a value is usable only when initialized on every
continuing predecessor; a loop body must restore every moved loop-carried value
on every backedge.

Place overlap is field-sensitive for statically distinct struct fields. A whole
place overlaps every child. Enum payloads and unknown array indices initially
overlap their container. Known distinct indices may be recognized by a narrow
constant rule, but general symbolic inequality belongs to later optimization.

### 6. Non-lexical regions are inferred from uses and constraints

Every explicit or inserted borrow creates a loan and a fresh internal region
variable. Uses of the resulting reference, reborrows, assignments between
reference locals, returns, and calls add containment/outlives constraints. The
solver chooses the smallest CFG-point set satisfying those constraints rather
than the enclosing source scope. This makes a shared borrow end after its last
use and allows a later `&mut` without braces or lifetime spelling.

References are allowed only in locals, parameters, and results, so provenance
is a finite set of input places and projections. Callable summaries contain:

```text
ReturnProvenance {
    kind: Shared | Mutable,
    origins: BTreeSet<InputPlace>,
}
```

Direct and resolved method calls substitute caller places into callee origins.
Branches union origin sets. Recursive and mutually recursive callable summaries
use a deterministic work queue and monotone fixed point, like the existing
requirement analysis. If a returned borrow might originate from two inputs,
both remain conservatively borrowed. A mutable returned borrow conservatively
reserves all candidate origins exclusively.

Source annotations were rejected because the purpose of whole-program analysis
is to remove them. Inferring a function's owned/shared/mutable parameter mode
from its body was also rejected: changing an implementation would silently
change its public ownership contract. Only lifetimes and return origins are
inferred.

### 7. Control-flow constructs inherit one whole-value mode

Plain `match value`, `for item in array`, and non-Copy `optional ?? fallback`
borrow. Prefixing the scrutinee/iterable/left operand with `&mut` grants an
exclusive borrow where the construct supports mutation. Prefixing it with
`move` consumes the entire value. Every match arm or iteration inherits that
mode; pattern-level and field-level partial moves are not accepted.

For consuming `match`, the selected payload becomes owned in the arm and all
unselected owned contents are dropped. For consuming `for`, elements move out
in order and the buffer is released after completion. For consuming `??`, the
present payload moves out or the fallback is evaluated lazily. Copy values keep
their current direct-value behavior.

Whole-scrutinee modes were selected over Rust-style per-pattern moves because
they eliminate path-dependent partially moved aggregates and keep the first
checker explainable. Partial moves can be added without changing these forms if
real programs justify them.

### 8. Scope facts and drop actions are explicit analysis output

Type lowering assigns `ScopeId`s according to Rhodolite's existing lexical
rules. A second-class block reuses its surrounding scope; match arm bindings,
loop variables, provider bodies, callable bodies, and test bodies retain their
existing isolation. Ownership does not silently change name visibility.

Each owned local has an initialization point and one eventual owner. The
ownership plan places compiler-generated drops on every edge that exits its
scope, in reverse declaration order, excluding moved or never-initialized
places. `return` accumulates drops from every exited scope before transferring
its result. A runtime failure does not unwind language scopes. Because this
version has no user destructor, last-use storage release is permitted later as
an unobservable optimization, but the reference order is lexical.

An invocation-wide arena was rejected as the language contract because it
would retain dead buffers and would not model Rust-like deterministic resource
ownership. The future Wasm backend will use an instance-local allocator with
free and `memory.grow`; ordinary OOM will trap, with explicit fallible APIs
deferred. Those allocator details are not implemented in this change.

### 9. The interpreter uses owned locations, not semantic shared references

The interpreter becomes the reference implementation for the new semantics.
Owned compound values live in an interpreter store with stable location IDs;
environment slots hold owned values or statically checked reference places.
Move removes an owned value from its source slot, borrow carries a place without
ownership, mutation resolves an exclusive place, and drop recursively releases
the owned graph. Debug assertions may detect a violated compiler invariant, but
accepted programs do not perform runtime borrow checking.

This store is an interpreter implementation detail, not a GC or shared ownership
feature. It may use Rust containers internally for stable addressing, but an
unreachable location is removed at its planned drop. `clone()` walks the owned
graph and creates independent locations. Since shared ownership and aggregate
references are unavailable, accepted graphs are acyclic except through
`indirect` unique ownership and recursive drop/clone are well-defined.

Retaining the existing semantic `Rc<RefCell<_>>` behavior was rejected because
it would allow the reference evaluator to mask missed moves and would preserve
the alias semantics this change removes.

### 10. `indirect` breaks owned type-layout cycles without a `Box` type

AST and HIR field/payload declarations record an `indirect` bit. After named
types resolve, a type-containment graph includes direct owned struct fields,
enum payloads, optionals, and arrays; indirect edges are excluded from the
inline-layout graph. Every strongly connected component containing a cycle is
an error, with related spans showing the cycle edges. The full ownership graph
retains indirect edges for recursive clone and drop.

The interpreter represents an indirect child by an owned store location. The
future Wasm layout will use an owning `i32` offset and is permitted to remove
the allocation through escape analysis. Automatically inserting hidden
indirection was rejected because it would hide allocation and pointer chasing;
surface `Box<T>` was rejected as unnecessary wrapper noise for a language-level
layout property.

### 11. Existing ambient and Wasm plans consume ownership-checked HIR

Requirement analysis continues to reason about calls and providers, but value
providers now carry a resolved mode: shared, mutable, moved, or temporary-owned.
The ownership pass checks their loan for the provision scope before ambient
specialization. Type-only provisions carry no runtime owner. The ambient plan
retains stable callable/slot identities; future data lowering can add owned or
borrowed record fields without revisiting provider selection.

The scalar Wasm support pass runs only after ownership succeeds. Copy scalar
locals require no new representation, and syntax-only `let mut` does not alter
their Wasm lowering. Any reachable reference or owned non-scalar value remains
an unsupported-target diagnostic. Unreachable data remains allowed only after
its ownership semantics check successfully, matching the existing distinction
between whole-program language checking and reachable backend support.

### 12. Migration is a single semantic cutover

All maintained code migrates together:

- mutable locals gain `let mut`;
- functions and trait methods gain owned, `&`, or `&mut` parameter/receiver
  modes;
- mutating calls and providers gain `&mut`;
- consuming bound arguments gain `move`;
- old alias tests become move, borrow, conflict, and clone tests;
- the canonical program keeps the same successful business result under the
  new ownership contract.

No parser flag, edition switch, or legacy evaluator remains. Keeping both
models was rejected because every downstream operation would need an ownership
mode switch and the reference interpreter could no longer define one language
meaning.

## Risks / Trade-offs

- [Whole-program region inference becomes slow or fails to converge] → Use
  finite input-place origin sets, monotone summaries, deterministic work queues,
  and focused recursion/branch stress tests before migration.
- [The change is too large to keep stable] → Land syntax/types, move checking,
  borrow/provenance, expression integrations, interpreter semantics, and source
  migration as independently verified commits.
- [Context-sensitive access rules surprise users] → Render the selected
  parameter/receiver mode and the inserted shared borrow in diagnostics and HIR
  snapshots; never insert mutable borrow, move, or clone.
- [Second-class block scopes produce unexpected drop timing] → Preserve existing
  name scopes exactly, expose `ScopeId` and drop-plan snapshots, and document
  that braces alone do not create a new owner scope.
- [Conservative array and enum overlap rejects safe code] → Provide diagnostics
  naming both places and record field/constant-index improvements separately;
  do not add runtime checks or unsafe bypasses.
- [Borrowed returns from multiple inputs retain too many loans] → Preserve the
  safe provenance union first; allow later path or value specialization to
  shrink it without changing source contracts.
- [Interpreter location storage accidentally becomes observable identity] → Do
  not expose pointer equality or location IDs; equality remains structural and
  clone always produces independent ownership.
- [Recursive drop overflows the host stack] → Start with correct recursive
  semantics and include deep-chain tests; a later iterative drop walker can
  replace the implementation without changing the contract.
- [Canonical ambient providers need extensive receiver changes] → Migrate trait
  declarations, implementations, `with` modes, and calls in one checkpoint and
  retain requirement-plan snapshots after ownership checking.
- [Scalar Wasm regresses despite no data lowering] → Keep byte-determinism and
  engine execution tests for the existing scalar corpus at every integration
  checkpoint.

## Migration Plan

1. Add syntax and resolved type/receiver representations while rejecting new
   forms before execution; snapshot parsing and HIR.
2. Add Copy classification, mutability, moves, direct ownership modes, recursive
   layout validation, and their diagnostics without changing the interpreter.
3. Introduce CFG scopes/places and complete intra-body move and loan checking.
4. Add interprocedural return-provenance fixed points, calls, methods, and
   resolved provider modes.
5. Integrate `match`, `for`, `??`, field projections, clone, equality, and
   deterministic drop plans.
6. Replace interpreter compound aliasing with owned locations and make all new
   semantics executable.
7. Migrate the canonical program, examples, tests, documentation, requirement
   planning, and scalar Wasm build path in one repository-wide cutover.
8. Run formatting, full tests, canonical interpreter cases, ownership failure
   corpus, unchanged ambient-plan snapshots, scalar Wasm validation/execution,
   deterministic rebuilds, and strict OpenSpec validation before archiving.

Each completed step is committed as a stable snapshot. Before the semantic
cutover, rollback may revert the latest isolated checkpoint. After source
migration, rollback returns to the last pre-ownership snapshot as one unit; no
dual-semantics compatibility state is supported.
