## Context

`CheckedProgram` is now the boundary for both execution paths: it contains name-resolved, concretely typed HIR plus ownership modes and a per-body CFG/drop plan. The interpreter executes compound values through an internal store, while `src/wasm.rs` accepts the same boundary but rejects every reachable non-scalar type, reference, and data expression before emitting a scalar-only module.

The generated artifact must remain import-free Core Wasm. Source ownership is single-owner with inferred borrows, `indirect` is the only recursive storage edge, and cleanup order is already fixed before backend lowering. Aggregate-stored borrows, shared ownership, user destructors, trait/ambient code generation, and trap unwinding are not available. See `proposal.md` and the three delta specs for the observable contract.

## Goals / Non-Goals

**Goals:**

- Make layout and lowering a deterministic function of checked HIR IDs and the ownership plan.
- Keep the ordinary function ABI small while correctly distinguishing Copy values, owned handles, and borrowed addresses.
- Reclaim owned buffers during normal execution without a host runtime or tracing mechanism.
- Make the public rich-value ABI memory-safe at the language boundary without exposing the internal heap layout.
- Land the backend as vertical slices in roadmap order: `str`, struct, enum/optional/match, then array/for.

**Non-Goals:**

- A stable cross-compiler internal heap layout or direct host access to compiler-owned values.
- Aggregate borrows, partial moves, RC/GC, user allocation APIs, destructors, unwinding, or allocator tuning.
- Trait, method, slot, `with`, or non-empty ambient-record lowering; those remain in `compile-wasm-traits-and-ambient`.
- Component Model, WIT, WASI, JavaScript glue, or a second backend.

## Decisions

### 1. Add separate layout, runtime, and value-lowering modules

Keep module assembly and scalar control flow in `wasm.rs`, move public-boundary work through `wasm_abi.rs`, and add three backend modules:

- `wasm_layout.rs`: deterministic `Type -> Layout` planning, recursive glue identities, and public wire-type discovery.
- `wasm_runtime.rs`: linear memory, allocator, exchange buffer, primitive memory operations, and emitted helper functions.
- `wasm_data.rs`: typed value representation, locals, places, constructors, projections, clone/equality/drop glue, and data control-flow lowering.

These modules consume `hir::Type`, resolved declaration IDs, and `ownership::Plan`; none may repeat name resolution, type inference, borrow checking, or call selection. `wasm.rs` remains the composition root and assigns all type/function/data indices in deterministic arena order.

Keeping everything in the existing emitter was rejected because scalar stack lowering, recursive layout, generated glue, and allocator instruction encoding would share one mutable namespace and make later ambient work harder to isolate. Introducing a general compiler IR was also rejected: the current typed HIR plus ownership plan contains the required semantics, and a new general IR would expand this change without another consumer.

### 2. Use flat Wasm values for Copy types and `i32` addresses for owned or borrowed data

The internal calling convention is derived from the checked type:

| Rhodolite value | Internal Core Wasm values |
|---|---|
| `unit` | none |
| `bool` | `i32` normalized to 0/1 |
| `int` | `i64` |
| fieldless enum | `i32` declaration-order tag |
| optional of a Copy value | `i32` tag followed by the recursively flattened Copy payload |
| owned non-Copy value | one nonzero `i32` address of its root allocation |
| `&T` / `&mut T` | one nonzero `i32` address of the checked place |

Locals and function signatures expand to zero, one, or several Wasm values from this table. Multi-result block types and functions are interned through the existing deterministic type table. A move of an owned value copies only its address and clears the source initialization flag; a borrow copies an address without acquiring ownership.

Using an `i32` box for every value was rejected because copying `int?` and other Copy optionals would either alias an allocation that is dropped twice or introduce hidden allocation/reference counting. Flattening every aggregate into function values was rejected because variable-length data and recursive values do not have a bounded useful signature.

### 3. Lay out owned roots inline and use addresses only at ownership boundaries

All memory offsets and sizes are unsigned 32-bit values checked with wider arithmetic during compilation. Alignment is a power of two and no greater than 8. Address zero is reserved as an invalid/uninitialized address.

- `bool`, `int`, and fieldless enum occupy 1, 8, and 4 bytes in memory respectively.
- A direct optional is `{u8 tag, padding, payload}`; tag 0 is empty and tag 1 is present.
- A struct stores fields inline in declaration order with normal alignment padding.
- A payload enum is `{u32 variant, padding, max-sized payload}`; only the active variant's payload is initialized.
- `str` is `{u32 data, u32 len, u32 capacity}` with UTF-8 bytes in a separate allocation.
- `[T]` is `{u32 data, u32 len, u32 capacity}` with elements at the planned element stride.
- A declared `indirect` field or payload is one `u32` child address. If its source type is optional, zero represents `nil`; otherwise zero is invalid.

Every non-Copy local/parameter/result owns or borrows an address to one root layout. Direct fields and payloads are inline below that root. Moving a direct nested projection allocates a new root for the extracted value, relocates it, drops the recorded remainder, and frees the consumed container; moving an indirect child can detach its existing address. Struct literal source evaluation order is retained by evaluating field values into temporary representations before installing them in declaration-order storage.

Representing every nested field as an independent pointer was rejected because it would erase the language-level distinction made by `indirect` and multiply allocations. Exposing Rust or host-native layout was rejected because build reproducibility and Core Wasm portability require an explicit target layout.

### 4. Emit a deterministic coalescing free-list allocator

ABI exchange/static data occupy a deterministic prefix of memory, followed by an aligned heap. Heap blocks carry a fixed header with payload size and free-list links. The allocator keeps the free list in address order, performs first-fit selection, splits a block only when the remainder can form a valid block, inserts freed blocks in address order, and coalesces adjacent free blocks. If no block fits, it grows memory by the minimum whole-page count needed and retries. Checked addition/alignment overflow or `memory.grow == -1` traps.

All allocator helpers are private Wasm functions. Only the ABI exchange reservation helper is exported, and it manages a distinct tracked exchange allocation through the same allocator. The implementation need not promise performance beyond deterministic reuse; tests cover alignment, split/coalesce, reuse in loops, growth, and failure edges.

A bump allocator was rejected because normal drops in a bounded loop would still grow memory until OOM. Shipping `dlmalloc`, WASI, or another runtime dependency was rejected because the allocator is part of the ownership story this project is intended to explain and the artifact must remain import-free. A size-class allocator was deferred until measurements show fragmentation or throughput matters.

### 5. Generate one clone, equality, and drop glue function per reachable layout

Layout planning interns structural glue keys in deterministic order. Recursive glue functions reserve their function indices before visiting children, using the same close-the-cycle technique as ambient specialization.

- `drop_T(ptr)` recursively drops only active/present contents, frees string/array buffers and indirect children, then frees the root when invoked as root glue.
- `clone_T(ptr) -> i32` allocates a fresh root and recursively clones initialized owned content. If a trap occurs, Rhodolite performs no language-level unwind, consistent with the existing contract.
- `eq_T(left, right) -> i32` compares tags, lengths, and content through shared addresses and short-circuits.

Inline child glue has an internal form that operates on addresses without freeing the enclosing root. Copy-only children use direct loads/stores and need no owning glue. Arrays call element glue using the planned stride; indirect edges recurse through child root glue.

Generating glue at every expression site was rejected because it duplicates code and makes recursive types awkward. A runtime type-descriptor interpreter was rejected because it adds dynamic dispatch and a second type system to generated modules.

### 6. Lower cleanup from the checked ownership plan, with one flag per conditional owner

`BodyPlan` gains a narrow read-only backend view that exposes access modes and cleanup events keyed to structured HIR exits; the ownership checker remains the only producer. Wasm lowering must not reconstruct liveness from syntax.

Each non-Copy owning local has an `i32` address slot and an `i32 initialized` flag. Binding or assignment sets the flag after the new value is complete; move clears it. Cleanup emitted for `Drop::Local` guards `drop_T` with this flag and clears it afterward. `Drop::Remaining` invokes projection-aware remainder glue once, transfers the selected value, and clears the root. Branch joins and loop iterations therefore carry runtime state only where the static plan permits different initialized states.

Returns are lowered through one function cleanup block: the result is first saved in result locals, exited-scope drops run in plan order, then the saved result is returned. Fallthrough, branch exits, and loop exits use the same cleanup-event emitter. `unreachable` traps do not branch through cleanup.

Deriving drop order from lexical scopes inside the backend was rejected because moves and control-flow joins have already been solved by ownership checking. Calling drop at last use was deferred because it is only legal when unobservable and is an optimization, not required semantics.

### 7. Lower data features as address operations over checked access modes

`wasm_data` provides a small typed interface: emit a value, emit a place address, move from a place, assign a place, and discard a produced value. `BodyPlan::access(expr)` selects copy/shared/mutable/move behavior; the emitter never infers the mode from syntax.

- Field and optional-field expressions compute checked offsets after evaluating the receiver once.
- Assignment evaluates the receiver place and new value, drops the old field only after the new value is ready, then installs the new representation.
- `??` and `match` test tags and bind payload addresses or relocated owned values according to the plan.
- Shared/mutable `for` walks array element addresses without structural mutation; consuming `for` clears each yielded element before its iteration cleanup and finally releases the buffer/root.
- String/array/compound equality and clone call their planned glue.

This layer is added vertically in the roadmap order. Each slice must execute in the standalone engine and retain the scalar tests before the next slice begins.

### 8. Keep ABI v0 for scalar public surfaces and add a serialized ABI v1

Internal rich layout is deliberately not a host contract. A module selects ABI v0 when all public signatures remain scalar; this preserves existing wrappers and metadata. Any owned public type selects ABI v1 module-wide.

ABI v1 flattens each rich parameter to `(i32 ptr, i32 len)` and each rich result to two Core Wasm results `(i32 ptr, i32 len)`. Scalars keep v0 representations. The module exports `memory` and `__rhodolite_abi_reserve(i32) -> i32`. The host stages canonical bytes in the most recently reserved exchange area. Wrappers bounds-check and decode complete values into fresh internal ownership before invoking the body. A rich result is encoded into the exchange area, its internal owner is dropped, and the bytes remain valid only until reserve or another function export invalidates them.

The wire encoding is recursive and self-delimiting by declaration type: little-endian `i64`, one-byte Boolean, no bytes for unit, UTF-8 string as `u32 byte_len + bytes`, struct fields in declaration order, enum as `u32 tag + active payload`, optional as `u8 tag + present payload`, and array as `u32 element_count + elements`. Decoders reject overflow, invalid tags/Boolean/UTF-8, truncation, and trailing bytes before the function body runs.

Passing internal heap pointers directly was rejected because it makes layout permanent, lets malformed alias graphs violate single ownership, and requires public manual-free operations. JSON was rejected because numeric/text ambiguities and parser size add no value to a typed binary boundary. Component Canonical ABI/WIT was again deferred: a downstream framework can translate ABI v1 bytes when it wraps the Core module.

### 9. Extend metadata with a canonical public wire-type graph

ABI v1 metadata keeps `version`, `entry`, and name-sorted `exports`, then adds `types`. Public signature types use scalar spellings or deterministic `t0`, `t1`, ... references. The type graph is discovered from entry then exports in canonical export-name order; within a declaration it follows field/variant/payload order. A type ID is reserved before its children are visited so recursive indirect types close without infinite traversal. Only types reachable from public signatures appear.

Each type entry records its kind and canonical wire schema: array element, optional payload, ordered struct fields, or ordered enum variants/payloads. It does not publish heap offsets, allocator headers, capacities, or glue indices. The exact compact JSON grammar and one recursive fixture are snapshot-tested and recorded in the ABI module documentation when implemented.

Publishing internal offsets was rejected because the serialized boundary intentionally permits internal layout changes without an ABI bump. Emitting every loaded type was rejected because private declarations are not part of the host contract and would make unrelated edits change metadata.

## Risks / Trade-offs

- **[The hand-written allocator can contain memory-corruption bugs]** → Keep it private, specify invariants in Rust-side model tests, validate generated modules, and run allocation/drop stress programs in the independent engine before enabling data lowering.
- **[Cleanup events do not currently have a public structured-backend view]** → Add only read-only accessors or a derived cleanup schedule to `ownership.rs`; snapshot it against existing plan dumps so the backend cannot silently invent ownership semantics.
- **[Relocating a moved direct field performs an allocation]** → Preserve the existing whole-container move contract first; optimize root reuse only after byte/result differential tests exist.
- **[Serialized ABI v1 adds encode/decode cost and a bounded result lifetime]** → Keep v0 for scalar surfaces, document invalidation in metadata/module docs, and leave zero-copy framework adapters for a later measured need.
- **[Exported linear memory lets a hostile host overwrite module state]** → State that hosts may write only the latest reserved exchange range; validate every input slice. Full isolation would require multi-memory or a component boundary and is outside this Core Wasm ABI.
- **[The change is large]** → Commit and verify after each vertical slice, preserving a stable scalar backend snapshot before proceeding.

## Migration Plan

1. Add layout planning and allocator/glue unit tests without changing supported programs; scalar module bytes remain the baseline.
2. Lower internal `str`, then struct operations, committing each verified slice.
3. Add enum/optional/match and then array/for, consuming checked cleanup events throughout.
4. Add ABI v1 encoding, validation, metadata, and rich wrapper tests while retaining ABI v0 selection for scalar surfaces.
5. Update ADR-0009 with a superseding ABI-v1/layout ADR, update the compiler roadmap status only when all scenarios and canonical differential checks pass, then archive the change.

Rollback is a revert to the last verified slice. Until the final archive, unsupported constructs continue to fail before artifact publication rather than falling back to partial generation.
