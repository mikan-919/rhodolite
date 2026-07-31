## Context

See `proposal.md` for motivation. The authoritative pipeline is
`load AST → check/lower HIR → analyze HIR → eval HIR`. HIR slot calls carry
`SlotId + TraitMethodId + SlotReceiver`, and each `with` provision already carries the
statically selected `TraitImplId` plus an optional value expression. The interpreter keeps a
`SlotId → AmbientBinding` map across calls and selects an implementation body at runtime.

Requirement analysis internally computes requirements by `BodyKey` and `SlotId`, but publishes
only a name-keyed view for diagnostics and display. A slot trait method's requirements are the
conservative union of every implementation body. This change preserves that observable contract:
specialization does not make previously rejected programs compile and does not change inferred
requirement output.

The program is whole-program, every provider implementation is known in HIR, and dynamic
first-class providers do not exist. These constraints make complete ambient monomorphization
possible without a vtable fallback.

## Goals / Non-Goals

**Goals:**

- Define an executable boundary between typed HIR plus requirement analysis and future C
  lowering.
- Enumerate only reachable callable instances, specialized by the concrete ambient
  implementations that their existing requirements need.
- Resolve every slot call in an instance to one concrete implementing `CallableId`.
- Represent runtime ambient transport as canonical minimal records containing only value-level
  provider handles.
- Preserve provider identity, lexical replacement, provision evaluation order, deterministic
  output, and termination through recursion.
- Make the complete canonical program mechanically lowerable to a deterministic specialization
  plan without reading the AST or emitting C.

**Non-Goals:**

- Provider-sensitive requirement inference or pruning requirements after implementation
  selection.
- Changing requirement display, missing-provider diagnostics, source behavior, or interpreter
  behavior.
- C emission, concrete data layout, allocation, ownership, async lowering, or a stable external
  ABI.
- Dynamic provider selection, vtables, fallback dispatch, function values, closures, or
  higher-order monomorphization.
- Exposing the plan through a new CLI option.

## Decisions

### 1. Requirement analysis exposes one semantic ID view and retains its presentation view

Refactor `requirement::Analysis` so the fixed-point result keyed by HIR identities remains
available after analysis. The semantic view is conceptually:

```text
BodyRequirements {
    body: BodyId
    slots: SlotId -> SlotRequirement {
        level: Type | Value
        span
        path
    }
}
```

Trait-method virtual bodies may remain an internal key used by the fixed point, but the
specialization planner consumes requirements for concrete `BodyId`s. The existing name-keyed
`reqs`, declaration order, rendering, paths, and diagnostics are derived from the same result and
remain byte-for-byte compatible.

Re-running requirement inference inside the planner was rejected because it would create two
authoritative algorithms. Converting public strings back into IDs was rejected because HIR was
introduced specifically to remove semantic name lookup.

### 2. Specialization is demand-driven from the entry and every test

The planner starts with the selected free entry callable and every `TestId`, each under an empty
provider context. It walks every reachable expression conservatively, including all conditional
branches, loop bodies, match guards and arms, provision expressions, and `with` bodies.

Direct, concrete method, and concrete associated calls request an instance of their resolved
`CallableId`. Slot calls first resolve the selected implementation body, then request an instance
of that concrete callable. Unreachable declarations remain fully type-checked but receive no
compiled instance.

Enumerating every callable times every compatible implementation combination was rejected because
it creates unused instances and makes the number of implementations, rather than reachable code,
control output size.

### 3. Instance identity contains implementations, never provider values

A callable instance is interned by a deterministic key equivalent to:

```text
InstanceKey {
    callable: CallableId
    providers: [(SlotId, TraitImplId)] // SlotId order, restricted to callable requirements
}
```

The requirement level is fixed by the callable's existing requirement result and therefore need
not be duplicated in the key. Type- and value-level requirements both contribute their selected
`TraitImplId`, because either can change the direct call targets in the specialized body.

The actual runtime value, the expression that produced it, and the call site are not part of the
key. Thus two calls using different `InMemoryDb` objects share one code instance while receiving
different handles in their ambient records.

Specializing by the entire caller context was rejected because unrelated slots would duplicate
code. Specializing by requirement shape without `TraitImplId` was rejected because slot call
targets would remain dynamic.

### 4. Existing conservative requirements remain the ABI contract for this change

The current analyzer unions ambient requirements from every implementation of a trait method. The
planner uses that existing result unchanged when restricting provider contexts and constructing
records, even when a selected implementation would need fewer slots.

For example, if one `Database::save` implementation uses `clock` and another does not, a source
function calling `db.save` continues to carry `db + clock` in every specialization. The selected
implementation call is still direct, but an unused `clock` field can remain in that instance's
record.

Making requirements provider-sensitive was rejected for this change because it would change
accepted programs, displayed requirements, and missing-provider paths while combining a core
inference redesign with the ABI boundary. The plan's deterministic output will make such excess
measurable for a later change.

### 5. Provider context separates compile-time selection from runtime value source

While walking one root or instance, the planner maintains:

```text
ProviderBinding {
    implementation: TraitImplId
    value_source: None | IncomingField | ProvisionValue
}

ProviderContext = SlotId -> ProviderBinding
```

`None` is a type provision. A value source is present for a value provision. A value binding can
satisfy a type-level requirement because it also selects an implementation; a type binding cannot
satisfy a value-level requirement.

Before creating a callee instance, the planner restricts the current context to the callee's
requirements, verifies every required level is available, and interns the resulting
`InstanceKey`. Missing or incompatible bindings after successful frontend validation are reported
as a structured planning error rather than silently generating a partial plan.

### 6. Type providers disappear; value providers use canonical record layouts

Type-level requirements affect the `InstanceKey` and direct call selection but occupy no runtime
field. Value-level requirements create one field holding a stable handle to the concrete provider
struct selected by their `TraitImplId`.

A record layout is interned by its ordered runtime fields:

```text
RecordLayoutKey = [(SlotId, StructId)] // SlotId order, value requirements only
```

Instances with identical runtime fields share a layout even if their type-only implementation
selections differ. An instance with no value-level requirements has no hidden runtime ambient
argument.

A unified `{ self, vtable }`, nullable value fields, and runtime tags were rejected because the
implementation and requirement level are already known statically. Keeping type providers as
zero-sized fields was rejected because they transport no runtime information.

### 7. Ambient records are immutable values containing identity-preserving handles

The logical call convention passes the callee's ambient record by value. Each field is a handle to
the same provider object supplied by `with`; constructing, projecting, copying, or passing a record
never copies the provider object itself. Mutation through a provider remains visible through every
alias, matching the HIR interpreter's shared struct semantics.

This phase deliberately leaves the physical handle representation abstract. The compiled-v1
candidate is a pointer to a process-lifetime arena allocation, but the later data-layout change may
choose another stable handle without changing plan semantics.

Passing a borrowed pointer to a caller's stack record was rejected as the semantic ABI because its
lifetime would leak into recursion and future async lowering. Copying provider objects into records
was rejected because it would break identity and canonical shared-mutation tests.

### 8. Slot calls become direct calls with explicit receivers

For each `Call::Slot`, the current provider binding supplies a `TraitImplId`.
`Program::implementation_of(implementation, method)` yields the concrete implementing
`CallableId` and the plan records a direct call edge to its specialized instance.

- A value receiver loads the provider handle from the current record or local provision and passes
  it as the concrete method's `self`.
- A type receiver passes no provider value; its implementation choice exists only in the
  specialization context.
- Requirements of the concrete implementation callable are supplied by a projected callee record
  using the same rules as any other call.

There is no vtable and no switch on an implementation ID at runtime. A hybrid fallback was rejected
because all HIR provisions are statically resolved and two dispatch conventions would add
complexity without serving a compiled-v1 program.

### 9. `with` evaluates outside, then creates one immutable snapshot

For:

```text
with db(make()), clock<Frozen> { body }
```

the planner and future lowering perform these semantic steps:

1. plan and evaluate every value provision in source order under the unchanged outer context;
2. clone the outer provider context;
3. replace all named slots with their selected implementation and optional value source;
4. plan the body under the completed inner context;
5. discard the inner context on leaving the body.

No provision in the same `with` sees another provision from that list. Nested `with` replaces the
same slot rather than mutating an outer record. Calls inside the body construct their canonical
callee record by selecting handles from the inner snapshot.

Sequentially installing provisions was rejected because it differs from the interpreter and
requirement analyzer. Mutating one shared record was rejected because lexical restoration,
recursion, and future suspension would become stateful.

### 10. A worklist and early interning make recursion finite

When an `InstanceKey` is first requested, the planner allocates its `InstanceId` and marks it
pending before walking the body. A recursive edge to the same key reuses that ID. A worklist later
finishes every pending instance.

Nested provisions may produce another implementation combination and therefore another instance,
but the state space is finite: the program has finite callables, slots, and `TraitImplId`s, and
provider values do not participate in keys. Mutual recursion uses the same mechanism and requires
no separate calling convention.

Inlining recursive traversal before interning was rejected because it would not terminate.
Arbitrary depth or instance-count limits are deferred until real programs show a useful bound;
deterministic interning makes growth observable first.

### 11. The plan is a backend-neutral, deterministically rendered internal IR

Add an internal module containing at least:

- `Plan`, roots, `InstanceId`, `InstanceKey`, and specialized callable instances;
- `RecordLayoutId`, canonical layout keys, and ordered fields;
- planned call-site targets and ambient projections keyed by the owning body and `ExprId`;
- planned `with` transitions and value sources;
- structured planning errors;
- a deterministic renderer used by focused snapshots.

All maps that affect rendering or allocation order use HIR declaration order or ordered
collections. Display names are recovered from `hir::Program` only in the renderer; semantic plan
edges contain IDs.

The plan remains structured metadata over HIR rather than cloning and rewriting every HIR
expression. A future C emitter consumes HIR for ordinary expression shape and consults the plan for
specialized call targets, record construction, and provider receivers.

Creating a C-flavored low-level IR now was rejected because scalar control flow and data layout
belong to later roadmap phases. Keeping only handwritten pseudocode was rejected because it would
not prove the canonical program mechanically lowers.

### 12. Pseudo-C documents the contract without fixing data layout

The ADR includes examples equivalent to:

```c
typedef struct {
    Handle_InMemoryDb db;
    Handle_Frozen clock;
} Ambient_db_InMemoryDb_clock_Frozen;

bool handle__db_InMemoryDb__clock_Frozen(
    Ambient_db_InMemoryDb_clock_Frozen ambient,
    int64_t id
);

void stamp__db_InMemoryDb__clock_Frozen(
    Ambient_db_InMemoryDb_clock_Frozen ambient,
    Handle_User user
) {
    Frozen_now(ambient.clock);
    InMemoryDb_save(ambient.db, user);
}
```

The spelling is illustrative. The normative internal facts are field order, concrete provider
type, by-value immutable record semantics, identity-preserving handles, and direct call targets.

### 13. Async remains out of scope but the record model does not borrow stack state

An eventual async state machine can copy an immutable ambient record into its frame. Its handles
must keep referring to stable provider objects; process-lifetime arena allocation satisfies this
for compiled v1. Task inheritance, detached task lifetime, cancellation, cross-thread mutation, and
`Send`-like constraints remain future language decisions.

The plan must not encode a pointer to a temporary caller record or claim that provider storage is
owned by the `with` stack frame. No async syntax or runtime is added in this change.

## Risks / Trade-offs

- [Conservative requirements leave unused record fields after specialization] → Preserve language
  behavior now, expose deterministic plans, and measure before proposing provider-sensitive
  inference separately.
- [Instance combinations grow exponentially] → Generate only reachable keys, exclude values and
  unrelated slots from keys, intern before traversal, and snapshot instance counts.
- [The planner duplicates requirement logic] → Expose the analyzer's ID result and consume it
  directly; do not recompute the fixed point.
- [The plan becomes premature C IR] → Keep ordinary expression structure in HIR and represent only
  specialization, records, transitions, projections, and resolved edges.
- [Record copying is mistaken for provider copying] → Name fields as handles, pin shared mutation
  semantics in the ADR, and test two records that refer to the same provider value.
- [Type provisions accidentally gain nullable runtime fields] → Derive layouts exclusively from
  value-level requirements and test type-only instances with no ambient argument.
- [Recursive planning loops or produces unstable IDs] → Intern pending keys before walking and use
  deterministic root, expression, slot, and implementation order.
- [Internal planner failures surface as panics] → Return structured invariant errors and test
  missing/incompatible synthetic contexts independently of the normal validated path.

## Migration Plan

1. Preserve the ID-keyed fixed-point result in requirement analysis and prove existing rendering
   and diagnostics unchanged.
2. Add specialization and record vocabulary with deterministic renderers, without connecting it to
   CLI execution.
3. Add demand-driven planning for roots, ordinary calls, and concrete callable requirements.
4. Add provider contexts, `with` transitions, record projections, and direct slot-call resolution.
5. Add recursion, mutual recursion, deduplication, and deterministic full-plan snapshots.
6. Prove the canonical entry and tests lower to the expected provider-specific instances.
7. Record the ABI decision in a new ADR and update the compiler roadmap to link the active change.

Each stable step runs formatting, the full test suite, strict OpenSpec validation, and a canonical
plan snapshot before being committed. Rollback is commit-by-commit; the interpreter remains the
authoritative execution path throughout this change.

## Open Questions

- The concrete provider handle encoding is deferred to `compile-data-values`; it must preserve the
  stable identity assumed here.
- Exact generated C type and symbol spelling is deferred to the emitter; it must be deterministic
  and derived from plan IDs rather than becoming a public ABI.
- Async task inheritance and provider lifetime beyond process-lifetime arena storage are deferred
  until async enters the language roadmap.
