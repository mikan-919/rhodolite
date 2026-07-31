## 1. Preserve Semantic Requirements

- [x] 1.1 Add an ID-keyed requirement result for every concrete `BodyId`, retaining `SlotId`,
  `SlotLevel`, use span, and propagation path from the existing fixed-point computation
- [x] 1.2 Expose read-only lookup and deterministic iteration needed by later lowering without
  exposing private trait-method virtual keys
- [x] 1.3 Derive the existing name-keyed render and missing-provider diagnostics from the same
  semantic result, preserving text, ordering, labels, help, and related spans
- [x] 1.4 Add tests for free functions, implementation methods, tests, type/value levels, and
  multi-module IDs, plus regression tests proving current provider-conservative requirements remain
  unchanged
- [x] 1.5 Run formatting, the full test suite, canonical success and missing-provider CLI cases, and
  strict OpenSpec validation, then commit the semantic requirement API as a stable snapshot

## 2. Ambient ABI Plan Vocabulary

- [ ] 2.1 Add `src/ambient_abi.rs` with `Plan`, root and instance IDs, `InstanceKey`, provider
  selections, record layout IDs, record fields, value sources, planned calls, and structured
  planning errors
- [ ] 2.2 Intern instance keys by `CallableId` plus requirement-restricted, `SlotId`-ordered
  `TraitImplId` selections while excluding provider values and unrelated caller slots
- [ ] 2.3 Intern record layouts by `SlotId`-ordered value requirements and concrete provider
  `StructId`, omitting type-only requirements and the hidden argument for empty layouts
- [ ] 2.4 Add deterministic program-aware rendering that translates IDs to names only at the
  display boundary
- [ ] 2.5 Add focused vocabulary tests for key equality, distinct implementation selections,
  record-layout sharing, type-only erasure, value-handle fields, and deterministic ordering
- [ ] 2.6 Run formatting, the full test suite, and strict OpenSpec validation, then commit the plan
  vocabulary as a stable snapshot

## 3. Demand-Driven Callable Planning

- [ ] 3.1 Add a planning entry point that accepts typed HIR, semantic requirements, the selected
  entry callable, and all test roots under empty provider contexts
- [ ] 3.2 Walk every structured HIR expression conservatively, including branches, loops, match
  guards and arms, provision expressions, and nested bodies, without consulting the AST
- [ ] 3.3 Plan direct functions, concrete methods, and concrete associated calls as edges to
  requirement-restricted specialized instances with canonical callee record projections
- [ ] 3.4 Generate only root-reachable instances, deduplicate identical keys reached from different
  call sites and roots, and omit unreachable declarations
- [ ] 3.5 Return structured invariant errors when a synthetic provider context lacks a required
  slot or supplies only a type binding for a value requirement
- [ ] 3.6 Add tests for multiple roots, unreachable functions, unrelated caller slots, shared
  instances with different runtime provider values, conservative branch traversal, and invalid
  synthetic contexts
- [ ] 3.7 Run formatting, the full test suite, and strict OpenSpec validation, then commit
  demand-driven callable planning as a stable snapshot

## 4. Providers, Slot Calls, and `with`

- [ ] 4.1 Track each current provider as a concrete `TraitImplId` plus no value, an incoming record
  field, or a provision value expression
- [ ] 4.2 Resolve every slot type/value call through `TraitImplId + TraitMethodId` to the concrete
  implementing `CallableId`, with no vtable, switch dispatch, or source-name lookup
- [ ] 4.3 Pass the current concrete provider handle as `self` for value slot calls, omit it for type
  slot calls, and plan the concrete implementation callable's own ambient projection
- [ ] 4.4 Plan all value provision expressions in source order under the unchanged outer context,
  then apply all provisions together to an immutable inner snapshot
- [ ] 4.5 Represent nested replacement and callee projection without mutating or borrowing a
  caller's ambient record, while preserving provider identity across copied records
- [ ] 4.6 Add tests for type-only providers, value providers, value-satisfies-type projection,
  nested replacement, simultaneous provisions, provision expressions that use outer providers,
  slot receiver passing, and direct implementation targets
- [ ] 4.7 Run formatting, the full test suite, canonical success and missing-provider CLI cases, and
  strict OpenSpec validation, then commit provider and `with` planning as a stable snapshot

## 5. Recursion and Complete Program Evidence

- [ ] 5.1 Allocate and mark each new instance pending before walking its body, then complete pending
  instances through a deterministic worklist
- [ ] 5.2 Reuse the same instance for direct recursion and same-context mutual recursion, and create
  finite distinct instances when nested providers change the implementation combination
- [ ] 5.3 Add recursive, mutually recursive, nested-provider-cycle, and cross-root deduplication
  tests that pin instance counts and call edges
- [ ] 5.4 Add a complete canonical plan snapshot covering `main`, every test, production and test
  provider combinations, concrete slot-call targets, record layouts, projections, and type erasure
- [ ] 5.5 Add a focused example proving conservative requirements may retain an unused record field
  after concrete implementation selection, so provider-sensitive pruning is not introduced
  accidentally
- [ ] 5.6 Run formatting, the full test suite, canonical success and missing-provider CLI cases, and
  strict OpenSpec validation, then commit the complete deterministic planner as a stable snapshot

## 6. Record the ABI Boundary

- [ ] 6.1 Add ADR-0008 documenting whole-program ambient specialization, no vtable fallback,
  implementation-keyed instances, type-provider erasure, immutable by-value records, and
  identity-preserving provider handles
- [ ] 6.2 Include pseudo-C for production and test provider combinations, nested `with`, type-only
  provision, direct trait implementation calls, and recursive record passing
- [ ] 6.3 Document that concrete handle layout, C symbol spelling, async task inheritance, detached
  lifetime, and cross-thread mutation remain deferred, while ambient records must be capturable as
  values rather than borrowed stack state
- [ ] 6.4 Update `README.md`, `docs/overview.md`, and `docs/compiler-roadmap.md` to describe the
  specialization-plan boundary and leave C emission as the next phase
- [ ] 6.5 Run `cargo fmt --check`, the full test suite, canonical success and missing-provider CLI
  cases, and `bunx @fission-ai/openspec validate --all --strict`
- [ ] 6.6 Commit the completed ambient ABI plan and documentation as an archive-ready stable
  snapshot
