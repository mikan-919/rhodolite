## 1. Baseline and layout contract

- [x] 1.1 Add scalar Wasm byte snapshots and standalone-engine fixtures that lock the pre-change ABI v0 output before refactoring the emitter
- [x] 1.2 Add `wasm_layout.rs` with checked size/alignment arithmetic and deterministic layout identities for scalars, fieldless enums, and Copy optionals
- [x] 1.3 Extend layout planning to direct structs, payload enums, owned optionals, strings, arrays, and declared indirect edges, including recursive-ID reservation
- [x] 1.4 Add layout unit tests for padding, declaration order, nested optionals, recursive indirection, deterministic traversal, and 32-bit overflow diagnostics
- [x] 1.5 Record the internal layout, allocator, and serialized ABI v1 decisions in a new ADR that supersedes only the affected ABI-v0 limits in ADR-0009 and ADR-0010

## 2. Import-free ownership runtime

- [x] 2.1 Add `wasm_runtime.rs` and deterministic function/index plumbing for memory operations and private runtime helpers
- [x] 2.2 Emit the reserved memory prefix, nonzero heap base, block headers, and aligned first-fit allocation with checked split behavior
- [x] 2.3 Emit address-ordered free insertion and adjacent-block coalescing, then cover allocation reuse and fragmentation with model and engine tests
- [x] 2.4 Emit minimum-page `memory.grow` retry and trap paths for arithmetic overflow, failed growth, and exhausted 32-bit address space
- [x] 2.5 Validate that owned-runtime modules remain import-free, deterministic, independently valid, and bounded allocation/drop loops reuse memory

## 3. Typed data lowering and checked cleanup

- [x] 3.1 Add the flat Copy/owned-address/borrowed-address representation planner and expand function params, results, block types, and locals to deterministic Wasm value sequences
- [x] 3.2 Expose a narrow read-only backend cleanup view from `ownership::BodyPlan`, with snapshots proving it preserves the existing access modes and edge drop order
- [x] 3.3 Add `wasm_data.rs` interfaces for emitting values and places, moving, assigning, discarding, loading/storing Copy layouts, and relocating inline owned layouts
- [x] 3.4 Give every non-Copy owner an address slot and initialization flag, and lower checked `Drop::Local` plus move flag transitions
- [x] 3.5 Lower `Drop::Remaining` for consuming nested projections without double-dropping the selected path
- [x] 3.6 Route fallthrough, return, branch exit, and loop exit through plan-ordered cleanup while leaving traps non-unwinding
- [x] 3.7 Add engine tests for conditional initialization, move across branches, nested early return, loop cleanup, reverse declaration order, and consuming field projection

## 4. String vertical slice

- [x] 4.1 Emit deterministic UTF-8 literal data and construct owned `str` roots with independent heap buffers
- [x] 4.2 Generate and intern string drop, deep-clone, and short-circuit equality glue
- [x] 4.3 Lower string locals, assignment, function arguments/results, shared reads, explicit move, and `clone()` through the checked representations
- [x] 4.4 Add standalone-engine tests comparing string construction, calls, moves, clones, equality, cleanup, and OOM traps with the interpreter

## 5. Owned struct vertical slice

- [x] 5.1 Lower zero-field and field-bearing struct construction while preserving source evaluation order and declaration-order storage
- [x] 5.2 Lower Copy field reads, shared/mutable owned-field places, optional-field propagation, and field assignment with drop-before-install semantics
- [x] 5.3 Generate recursive struct drop, clone, and equality glue for direct and `indirect` fields
- [x] 5.4 Lower whole-struct moves and consuming direct/indirect field projections using the checked remainder plan
- [x] 5.5 Add engine tests for nested mutation, deep clone independence, structural equality, recursive `indirect` chains, moves, and exact cleanup

## 6. Enum, optional, coalesce, and match slice

- [x] 6.1 Lower fieldless enum tags and Copy optionals in flat Wasm values without heap ownership
- [x] 6.2 Lower payload-enum and non-Copy optional construction with initialized tags and active payload storage
- [x] 6.3 Generate tag-directed drop, clone, and equality glue for payload enums and owned optionals, including indirect recursive payloads
- [x] 6.4 Lower borrowed and consuming `??` with short-circuited fallback and correct ownership transfer
- [x] 6.5 Lower exhaustive/catch-all `match`, payload bindings, guards, and shared/mutable/consuming subject modes with single subject evaluation
- [x] 6.6 Add engine tests for invalid inactive storage avoidance, guarded arm order, borrowed and consuming payloads, recursive enums, coalesce, clone, equality, and cleanup

## 7. Array and iteration slice

- [x] 7.1 Lower empty and populated array literals with checked length/capacity arithmetic, element stride, and source-order initialization
- [x] 7.2 Generate array drop, deep-clone, and short-circuit equality glue for Copy and owned element layouts
- [x] 7.3 Lower shared `for` iteration over Copy and borrowed non-Copy elements without consuming the array
- [x] 7.4 Lower mutable `for` iteration with exclusive element places and no structural mutation during iteration
- [x] 7.5 Lower consuming `for` iteration by clearing yielded elements, running per-iteration cleanup, and releasing the exhausted buffer/root once
- [x] 7.6 Add bounded-loop reuse and interpreter-comparison engine tests for nested arrays, optional elements, all iteration modes, clone, equality, move, and cleanup

## 8. Serialized Rhodolite ABI v1

- [x] 8.1 Generalize ABI signature planning so scalar-only public surfaces retain ABI v0 and any owned public type selects ABI v1 module-wide
- [x] 8.2 Reserve the `memory` and `__rhodolite_abi_reserve` export names and emit a tracked exchange allocation with the specified invalidation lifetime
- [x] 8.3 Implement canonical encoders for scalars, strings, structs, enums, optionals, arrays, and recursive indirect values
- [x] 8.4 Implement bounded canonical decoders that reject overflow, out-of-range slices, invalid Boolean/UTF-8/tags, truncation, and trailing bytes before body execution
- [x] 8.5 Emit ABI v1 wrappers with mixed scalar and rich parameters, rich multi-value results, fresh internal ownership on decode, and result cleanup after encoding
- [x] 8.6 Extend canonical ABI metadata with deterministic public type-graph IDs, recursive references, wire schemas, and flattened Core Wasm signatures
- [x] 8.7 Add host-style engine tests for staging, mixed signatures, nested/recursive round trips, malformed inputs, result invalidation, private-type omission, and metadata determinism
- [x] 8.8 Prove with regression snapshots that scalar-only public modules still select ABI v0 and preserve their scalar signatures and metadata

## 9. End-to-end verification and documentation

- [x] 9.1 Add maintained owned-data Wasm fixtures that cover every new construct and compare results/final mutation state with the HIR interpreter
- [x] 9.2 Run the full Rust test suite, strict OpenSpec validation, independent Wasm validation/execution, repeated byte-determinism builds, and formatter/lint checks
- [x] 9.3 Update README and compiler roadmap to describe the owned-data backend, ABI v0/v1 selection, remaining trait/ambient limitation, and the next change
- [ ] 9.4 Mark every verified task complete and leave the change apply-complete for spec sync and archive
