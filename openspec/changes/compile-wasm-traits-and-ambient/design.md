## Context

`CheckedProgram` already resolves every free, inherent, associated, trait, and slot call and fixes receiver/argument ownership plus per-body cleanup. `ambient_abi::plan_production` already walks production roots from empty provider contexts and produces deterministic specialized instances, runtime-record layouts, call targets, receiver sources, and callee projections. The Wasm backend consumes those instances but currently rejects callable receivers, method/associated/slot calls, `with`, and every non-empty ambient layout.

Owned non-Copy values and internal borrows already use one nonzero `i32` root address, with layouts, allocator, clone/drop glue, and normal-edge cleanup generated from checked HIR. Core Wasm has no source-level struct parameter, so ADR-0008 intentionally leaves the physical ambient-record representation to this stage. The artifact must remain import-free, deterministic, and ABI v0/v1 compatible. See `proposal.md` and the two delta specs for the observable contract.

## Goals / Non-Goals

**Goals:**

- Make `ambient_abi::Plan` the only source of specialized function identity, slot implementation selection, call target, receiver source, and record projection during emission.
- Reuse the checked value representation and ownership cleanup instead of adding a second provider object model.
- Preserve exact evaluation and lifetime boundaries for method receivers and provision values.
- Land in vertical slices that leave a verified backend snapshot after methods, trait calls, value provisions, type provisions, and nested ambient forwarding.

**Non-Goals:**

- Dynamic providers, first-class trait objects, vtables, runtime implementation IDs, or late-bound dispatch.
- Aggregate-stored borrows, RC/GC, raw pointers, user destructors, or trap unwinding.
- Changing conservative requirement inference to depend on the selected provider.
- Exposing specialization names, ambient records, or provider injection through the public Rhodolite ABI.

## Decisions

### 1. Extend the internal function convention from each specialized instance

Replace callable-only signature lowering with instance signature lowering. Its logical inputs are:

1. receiver, when the callable declares one;
2. declared parameters in source order;
3. one `i32` provider handle per field in the instance's `RecordLayout`, in `SlotId` order.

Receiver and ordinary values keep the existing `Type -> Vec<ValType>` representation; every provider field is an `i32` because a slot implementation type is a struct and owned or borrowed struct access already lowers to a root address. The hidden fields are flattened into trailing Core Wasm parameters rather than allocating a record in linear memory. This is ADR-0008's immutable record passed by value at the physical Wasm boundary: its field order and projection are preserved, while no lifetime-bearing pointer to a caller stack record is introduced.

An empty layout adds no parameters. Type-only requirements affect the instance key and direct targets but add no field. Flattening was chosen over heap allocation because records never escape, have fixed scalar fields, and should neither allocate nor require cleanup. Passing one pointer to a memory record was rejected because it creates transient storage and an unnecessary lifetime to manage.

### 2. Keep function indices identical to `InstanceId` order

Reserve one implementation function for every production-plan instance in allocation order, including multiple instances of the same callable under different provider combinations. Method, associated, and slot calls obtain their target only from `instance.calls[expr_id].target`; no emitter lookup by callable name, trait ID, or implementation ID is permitted.

The existing wrapper-to-root mapping continues to call the selected root instance. Public roots start with an empty provider context, so public ABI signatures never expose hidden ambient fields. Early interning in `ambient_abi` already closes recursion and mutual recursion; preserving instance order keeps recursive call indices available before bodies are emitted.

Re-keying emitted functions by `CallableId` was rejected because it aliases different provider specializations. Recomputing dispatch during lowering was rejected because it duplicates the planner and could diverge from requirement diagnostics.

### 3. Add a focused ambient lowering layer beside data lowering

Add `wasm_ambient.rs` for backend-only mechanics:

- mapping incoming `SlotId` fields to hidden local indices;
- reserving typed temporary locals and initialization flags for value provisions;
- materializing a `ValueSource::Incoming` or `ValueSource::Provision` as one provider handle;
- emitting a planned callee projection in layout order;
- validating planner/backend invariants with structured diagnostics before module assembly.

`wasm.rs` remains the composition root and expression/control-flow emitter. `wasm_data.rs` remains the authority for value/place representation and clone/drop operations. `wasm_ambient.rs` must not resolve names, choose implementations, infer ownership, or invent cleanup order.

Keeping all provider bookkeeping inside `wasm.rs` was rejected because `with` lifetimes, hidden parameters, and ordinary data temporaries would share implicit index arithmetic. Extending `ambient_abi` to emit Wasm was rejected because the planner is backend-independent and its tests intentionally describe a physical-layout-neutral contract.

### 4. Lower ordinary method forms through one planned-call path

For `Method`, evaluate the receiver first using its checked access mode, then evaluate arguments in source order, append the planned ambient projection, and call the planned target. For `Direct` and `Associated`, evaluate declared arguments, append the projection, and use the same call helper. The callee maps an explicit receiver to `Body::receiver` before declared parameters and applies the existing ownership initialization/cleanup rules to an owned receiver.

For `Slot`, argument evaluation remains in source order. A value receiver is loaded from `PlannedCall.receiver` and passed before declared arguments; a type receiver has no runtime value. In both cases the target and ambient projection come only from `PlannedCall`. The lowering layer checks that receiver presence matches the selected callable's declared receiver and reports an internal plan diagnostic instead of panicking if an earlier invariant is broken.

Separate lowering per call spelling was rejected because receiver ordering, ownership flags, result handling, and ambient forwarding would drift. A vtable call was rejected because all selections are statically known and ADR-0008 forbids it.

### 5. Evaluate and retain value provisions before changing context

For a `With` expression, lower every value provision under the surrounding instance context in source order. Store each resulting handle in a provision-local seat keyed by its expression ID. Only after all values are materialized does the body use the planned calls whose `ValueSource::Provision` references those seats. The planner already calculated inner substitutions and call projections; the emitter does not maintain or mutate a second provider map.

A borrowed provision stores the referenced root address. An owned temporary or moved provider stores its owned root address and an initialization flag. The backend consumes existing checked access information to distinguish those cases and uses data drop glue on every normal exit from the provision scope. If a consuming slot receiver transfers an owned provider, its flag is cleared before the call so later cleanup cannot double-drop it. Trap paths continue not to unwind, matching the existing ownership runtime contract.

Evaluating and immediately installing provisions one at a time was rejected because provisions in one head must all observe the outer context. Cloning provider roots into record fields was rejected because it changes identity, mutation visibility, and ownership.

### 6. Treat ambient records as immutable projections, not owned containers

Incoming provider parameters are borrowed or owned handles whose lifetime/ownership was established by the caller's checked provision. Forwarding a handle copies only the `i32` address; it does not acquire ownership and does not get a drop flag in the callee merely because it appears in the ambient record. The receiver local is the only place where a selected method's checked receiver mode is applied.

At each call, emit `PlannedCall.projection` in its recorded order. `Incoming(slot)` reads the current instance's hidden field; `Provision(expr)` reads the retained seat. The callee signature is checked against the projection width before emission. This preserves unrelated-slot pruning and allows conservative inference to retain a field without the backend trying to optimize it away.

Making each callee rebuild a heap record was rejected because projections are compile-time-known scalar shuffles. Giving every forwarded handle ownership was rejected because it would create multiple owners for one allocation.

### 7. Preserve public ABI and deterministic module assembly

ABI wrappers continue to target root `InstanceId`s and expose only declared parameters/results. Because roots begin with empty provider contexts, any unsatisfied ambient requirement still fails before emission with existing path diagnostics. Internal hidden parameters never appear in `rhodolite.abi` metadata.

Function type interning includes the expanded internal parameter vectors; function/index allocation remains instance order, followed by wrappers and runtime/glue. Provider bookkeeping alone does not require linear memory or allocator helpers, so scalar provider programs using zero-field structs remain import-free without forcing the owned-data runtime unless ordinary reached layouts require it.

Determinism tests compare repeated bytes and plan/function snapshots. Publishing specialization or provider layout metadata was rejected because these are compiler-private and would unnecessarily stabilize implementation details.

## Risks / Trade-offs

- **[Provision temporaries are expression-owned rather than ordinary HIR locals]** → Centralize their seats and flags in `wasm_ambient.rs`, snapshot their allocation, and test fallthrough, return, branch, loop, shadowing, and consuming-receiver cleanup.
- **[Flattened hidden fields can make internal signatures wide]** → Carry only value-level inferred requirements and measure widths; defer record-pointer compaction until a real Core Wasm limit or workload justifies its lifetime complexity.
- **[Receiver ownership and provider ownership can clear different flags]** → Drive both from the checked access/receiver mode and add exact allocator-reuse tests that detect leaks and double frees.
- **[Planner and emitter indices can silently disagree]** → Add a pre-emission validation pass for target existence, receiver shape, projection slots/types, and callee parameter width, returning diagnostics rather than panicking.
- **[Conservative trait requirements may carry unused handles]** → Preserve the current language contract and layout exactly; provider-dependent requirement refinement remains a separate change.
- **[The canonical program may expose an unsupported corner beyond ambient lowering]** → Add narrow vertical fixtures first, then use the canonical build as the final integration gate without weakening its source semantics.

## Migration Plan

1. Lock current scalar/owned-data byte snapshots and add instance-signature plus method-call tests without enabling `with`.
2. Enable inherent and associated calls, then trait implementation bodies and statically planned direct targets.
3. Add flattened ambient fields and value-slot forwarding, followed by value-provision retention and cleanup.
4. Add type-only provisions, multi-provision outer-context evaluation, nested shadowing, and recursive forwarding.
5. Build and execute the canonical production program, run strict validation/lints/determinism checks, update roadmap documentation, then archive the change.

Each slice remains independently buildable and testable. Rollback is a revert to the preceding verified snapshot; unsupported checks stay in place for forms not yet enabled during implementation.
