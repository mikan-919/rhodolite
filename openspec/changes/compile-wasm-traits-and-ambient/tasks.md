## 1. Baseline and instance calling convention

- [x] 1.1 Add pre-change byte snapshots and independent-engine fixtures for scalar and owned-data programs so method/ambient plumbing cannot silently change existing ABI v0/v1 output
- [x] 1.2 Add `wasm_ambient.rs` with deterministic incoming-field and provision-seat allocation keyed by `RecordLayout` slots and provision expression IDs
- [x] 1.3 Add a pre-emission validator for planned call targets, receiver presence, projection order/type, and callee hidden-parameter width, with tests for rejected inconsistent plans
- [x] 1.4 Replace callable-only signature lowering with instance signature lowering that orders receiver, declared parameters, and flattened `i32` ambient fields deterministically
- [x] 1.5 Extend signature/local snapshots to cover empty records, value records, type-only requirements, multiple slots, and two specializations of one callable

## 2. Inherent and associated method slice

- [x] 2.1 Map a callable receiver into `Body::receiver` before declared parameters and apply existing owned-address, borrowed-address, initialization-flag, and cleanup behavior
- [x] 2.2 Introduce one planned-call emission path that evaluates a concrete receiver before source-ordered arguments, appends the planned projection, and calls the target `InstanceId`
- [x] 2.3 Lower inherent `Method` and receiver-free `Associated` calls through the planned-call path and remove their reachable unsupported diagnostics
- [x] 2.4 Add independent-engine/interpreter comparison tests for `self`, `&self`, `&mut self`, associated functions, owned-data arguments/results, recursion, early return, mutation, and exact drop behavior
- [x] 2.5 Run the full suite, strict lints, validation, and repeated-byte checks and preserve the verified inherent-method slice as a git snapshot

## 3. Trait implementation and direct target slice

- [x] 3.1 Emit reachable `CallableOwner::TraitImpl` bodies as ordinary specialized instances using the same receiver and parameter convention
- [x] 3.2 Lower planned trait implementation targets without callable-name, trait-ID, implementation-ID, vtable, or runtime-switch lookup in the emitter
- [x] 3.3 Add plan/function-index snapshots proving one shared instance is emitted once and different provider implementation keys produce distinct deterministic instances
- [ ] 3.4 Add engine tests for shared, mutable, and consuming trait receivers with scalar and owned-data parameters/results, including recursive implementation calls
- [ ] 3.5 Run the full suite, strict lints, validation, and repeated-byte checks and preserve the verified trait-implementation slice as a git snapshot

## 4. Ambient fields and slot-call slice

- [ ] 4.1 Append one flattened `i32` hidden parameter per value-level `RecordLayout` field and expose deterministic `SlotId -> local index` lookup to the body emitter
- [ ] 4.2 Materialize `ValueSource::Incoming` fields and emit every planned callee projection in recorded layout order without allocating or cloning a record
- [ ] 4.3 Lower value-slot calls by passing the planned provider handle as the selected implementation receiver, followed by source-ordered arguments and the callee projection
- [ ] 4.4 Lower type-slot calls without a runtime receiver while retaining provider selection in the target specialization and projection
- [ ] 4.5 Remove reachable unsupported diagnostics for supported slot calls and non-empty ambient layouts while retaining source-positioned diagnostics for genuinely unsupported provider forms
- [ ] 4.6 Add engine tests for direct and transitive value requirements, type-only requirements, multiple slots, unused-slot pruning, mutation visibility, implementation substitution, recursion, and byte determinism
- [ ] 4.7 Run the full suite, strict lints, validation, and repeated-byte checks and preserve the verified slot-call slice as a git snapshot

## 5. Value provision and ownership slice

- [ ] 5.1 Reserve typed provision seats and initialization flags for each reachable `with slot(value)` expression without treating borrowed handles as owners
- [ ] 5.2 Lower all value provisions once in source order under the outer context, retain their stable handles, and make `ValueSource::Provision` read the corresponding seat
- [ ] 5.3 Reuse checked access/receiver modes to preserve shared, mutable, moved, and temporary providers and clear ownership flags when consuming slot receivers transfer a provider
- [ ] 5.4 Emit drop glue exactly once for owned provision temporaries and moved providers on normal block fallthrough, branch/loop exits, and function returns while leaving trap paths non-unwinding
- [ ] 5.5 Add allocator-reuse and interpreter-comparison engine tests for shared persistence, mutable visibility, moved-provider invalidation, temporary cleanup, consuming receivers, repeated slot use, and early exits
- [ ] 5.6 Run the full suite, strict lints, validation, and repeated-byte checks and preserve the verified value-provision slice as a git snapshot

## 6. Type provisions, multi-provision heads, and nesting

- [ ] 6.1 Lower `with slot<Type>` as a runtime-free provider selection whose implementation affects only specialized targets and keys
- [ ] 6.2 Ensure every value expression in one multi-provision head observes only the outer provider context before all replacements take effect together
- [ ] 6.3 Lower nested `with` projections so named slots shadow independently and outer selections remain available after the inner body
- [ ] 6.4 Add engine/interpreter tests for mixed value/type heads, same-head outer-context observation, partial multi-slot shadowing, nested restoration, recursion under shadowing, and no hidden field for type-only slots
- [ ] 6.5 Run the full suite, strict lints, validation, and repeated-byte checks and preserve the verified nested-provision slice as a git snapshot

## 7. Canonical end-to-end verification

- [ ] 7.1 Add a maintained trait-and-ambient Wasm fixture that exercises every new call/provision form with scalar and owned data and compares its results and final mutation state with the HIR interpreter
- [ ] 7.2 Build `examples/canonical.rd` through the CLI, validate the import-free module independently, invoke its entry export, and assert the canonical result
- [ ] 7.3 Add an equivalent provider-substitution fixture proving intermediate functions need no source-level forwarding changes and generated calls select the replacements
- [ ] 7.4 Verify repeated canonical and substitution builds are byte-identical and internal ambient parameters do not appear in ABI metadata or public export signatures

## 8. Documentation and completion

- [ ] 8.1 Update README and compiler roadmap to mark trait/ambient Wasm generation complete and identify `add-differential-execution` as the next change
- [ ] 8.2 Run formatting, the full Rust test suite, strict Clippy, strict OpenSpec validation, independent Wasm validation/execution, and repository-wide repeated-byte checks
- [ ] 8.3 Mark every verified task complete, review the implementation diff for plan/emitter duplication or ownership regressions, and leave the change ready for spec sync and archive
- [ ] 8.4 Preserve the final verified implementation and documentation state as a git snapshot before beginning the next roadmap change
