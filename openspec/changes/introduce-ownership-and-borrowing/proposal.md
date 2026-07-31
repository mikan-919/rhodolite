## Why

Rhodolite's compound values currently behave as implicitly shared mutable
references, which leaves allocation, aliasing, mutation, and reclamation costs
without a static contract. Before compiling data values to Wasm, the language
needs an ownership model that preserves script-like reads while making mutation,
ownership transfer, copying, and indirection visible and proving memory safety
without a garbage collector or source-written lifetime parameters.

## What Changes

- **BREAKING** Replace implicit shared compound-value semantics with single
  ownership, shared borrows (`&T`), exclusive mutable borrows (`&mut T`), explicit
  ownership transfer at call sites (`move`), and explicit deep copying
  (`clone()`).
- **BREAKING** Make `let` immutable, add `let mut`, make non-Copy local binding
  move by default, and require mutable access for reassignment and field updates.
- Infer borrow provenance and lifetimes across the whole loaded program; reject
  use-after-move, escaping borrows, and conflicting access before requirement
  analysis, interpretation, or Wasm support checking. No lifetime annotations,
  runtime borrow checks, raw pointers, or `unsafe` are introduced.
- Define owned structs, strings, arrays, payload enums, and optionals; deterministic
  compiler-generated destruction; a small explicit Copy set; and structural
  `clone()` and equality behavior.
- Add ownership modes to field access, calls, methods, `match`, `for`, `??`,
  returns, and `with` providers. Shared reads stay concise; mutation and
  consumption remain visible at the use site.
- Add `indirect` recursive ownership edges without exposing `Box<T>` in source
  types. Reject recursive value cycles that contain no indirect edge.
- Migrate the canonical program and all maintained examples and tests in one
  step. Do not retain the old implicit-alias compatibility mode.
- Defer references stored inside aggregates, shared ownership, user-defined
  destructors, fallible allocation APIs, raw pointers, `unsafe`, rich Wasm data
  lowering, and public borrowed ABI values.

## Capabilities

### New Capabilities

- `ownership-and-borrowing`: Owned values, Copy/move/clone, shared and mutable
  borrows, whole-program provenance inference, access conflicts, and diagnostics.
- `deterministic-destruction`: Compiler-generated drop order and cleanup across
  ordinary scope exit and structured control flow.
- `indirect-recursive-values`: Explicit owning indirection for finite recursive
  struct and enum layouts.

### Modified Capabilities

- `total-static-type-checking`: Successful checking also closes ownership,
  provenance, and borrow-safety obligations before later stages.
- `local-binding-type-annotations`: `let` becomes immutable, `let mut` is added,
  and binding/assignment gains Copy, move, borrow, and clone behavior.
- `function-signature-type-checking`: Parameters and results accept `&T` and
  `&mut T`, owned arguments require visible transfer, and borrowed results infer
  their source provenance.
- `method-call-type-checking`: Receivers distinguish `&self`, `&mut self`, and
  consuming `self`, with mutable and consuming receiver modes visible at calls.
- `struct-shape-checking`: Struct values become owned, field projection borrows
  non-Copy fields, mutation needs exclusive access, and consuming projection
  consumes the whole struct.
- `array-type-checking`: Arrays own their buffers and `for` distinguishes shared,
  mutable, and consuming iteration.
- `data-bearing-enums`: `match` borrows by default and supports whole-scrutinee
  mutable borrow or consumption without partial moves.
- `optional-core-type-checking`: Owned optionals and `??` distinguish borrowed,
  copied, and consuming extraction.
- `optional-field-access`: Optional projection observes the ownership mode and
  never creates an unchecked escaping alias.
- `basic-expression-type-checking`: Equality borrows operands and structural
  cloning is typed without implicit copying.
- `with-provision-type-checking`: Value providers may be shared-borrowed,
  mutably borrowed, moved, or owned temporaries for the provision scope.
- `core-wasm-build`: Ownership checking becomes a prerequisite while the current
  Wasm target continues to reject reachable non-scalar data and borrowed values.

## Impact

- Lexer, parser, AST, module resolution, typed HIR, diagnostics, type checking,
  requirement analysis ordering, the interpreter, and the Wasm build pipeline
  gain explicit ownership information and an ownership-analysis phase.
- Function, method, local binding, field, control-flow, and provider syntax
  changes incompatibly; the canonical program, examples, fixtures, snapshots,
  and CLI diagnostics must be migrated together.
- The analysis produces deterministic per-body access/drop plans and
  interprocedural return-provenance summaries for later Wasm data lowering.
- No allocator, garbage collector, reference counter, WasmGC dependency, unsafe
  escape hatch, rich public ABI, or shared-ownership runtime is added by this
  change.
