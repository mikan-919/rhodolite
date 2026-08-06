## Why

MAP-080's `Map<T>` trait needs a way to build the `[U]` result array from
inside ordinary, checked Rhodolite source — but today nothing can grow an
array after it is literally written as `[a, b, c]` or received as a
parameter: `array-type-checking`'s array Requirements cover literal typing,
invariant assignability, and `for`-loop ownership modes, and never mention
appending an element. The only way an array is currently constructed with a
computed length is the interpreter's/backend's own internal `array_literal`
codegen and `Vec::push` calls (`src/wasm.rs`, `src/eval.rs`) — neither is
reachable from source. ROADMAP.md's MAP-Q6 decision already fixed the
contract this gap needs: a `trait Push<T> { fn push(&mut self, x: T) }` with
a compiler-synthesized `impl<T> Push<T> for [T]`, called as `xs.push(y)`,
growing capacity by doubling and trapping (not erroring) on allocator OOM
per ADR-0011. Nothing today implements or tests that contract, and no
existing mechanism lets a declared trait `impl` have a body the compiler
supplies instead of one parsed from source — `array_clone`/`array_drop`
(`src/wasm_data.rs:849,868`) are structural per-layout glue invoked only by
the ownership-plan cleanup pass, never reached through user call-site method
resolution the way `xs.push(y)` must be.

## What Changes

- Add `trait Push<T> { fn push(&mut self, x: T) }` to the trait/effect
  declaration surface and register `impl<T> Push<T> for [T]` as a
  compiler-synthesized generic trait impl: its signature is checked and
  contract-conformant exactly like a parsed `impl`, but it carries no
  parsed body, and generic instantiation, requirement inference, and
  ownership planning all treat it as a leaf with no ambient requirements and
  a fixed move-once-argument effect on `x`.
- Resolve `xs.push(y)` through ordinary trait-method call resolution
  (`src/typecheck.rs` `resolve`/`conform_receiver`), but exempt this one
  compiler-builtin `&mut self` method from the receiver-modifier
  requirement `method-call-type-checking` currently states for every bound
  mutable receiver: the call auto-borrows `xs` exclusively without a
  call-site `&mut`, the same implicit convention `db.save(...)`'s
  ambient-slot dispatch already uses (`docs/grammar.md:472` vs. `:490`),
  rather than the parser's `&mut`-rewrite path bound locals otherwise
  require (`src/parse.rs` `qualified_place`, `src/typecheck.rs:4218`
  `conform_receiver`).
- Implement `push`'s runtime behavior once for the interpreter
  (`src/eval.rs`, where an array is already a plain `Vec<OwnedValue>`, so
  `push` is a direct `Vec::push`) and once for Core Wasm (`src/wasm.rs` /
  `src/wasm_data.rs`, extending the `{data, len, capacity}` buffer
  (ADR-0011 §1) with capacity-doubling growth: capacity 0 → 1 on first
  push, then double; a full buffer reallocates via the existing
  allocator's `alloc`, copies old elements the way `array_clone`
  already does (`src/wasm_data.rs:868-899`), and frees the old buffer).
- Reuse the existing allocator OOM contract for a failed grow: a `memory
  .grow` failure traps via `unreachable` (`src/wasm_runtime.rs`'s
  `extend`, ADR-0011 §3) with no new push-specific failure value or error
  classification.
- Add differential (interpreter vs. independent Wasm engine) coverage for
  `push`, including capacity-growth boundaries (0→1→2→4→...) and
  ownership final-state parity for a pushed non-`Copy` value.
- Explicitly keep `len()` and index access (`xs[i]`) out of scope
  (ROADMAP Future Directions); `push`'s own capacity/length bookkeeping
  is internal state with no new source-visible accessor.

## Capabilities

### New Capabilities
- `array-push`: the `Push<T>` trait, its compiler-synthesized `[T]` impl,
  `xs.push(y)`'s call-site and ownership contract, capacity-doubling growth,
  and OOM-traps-via-`unreachable` on realloc failure — checked identically
  and with matching final state in the interpreter and Core Wasm.

### Modified Capabilities
- `method-call-type-checking`: the "Receiver effects are visible at calls"
  Requirement currently states every bound mutable receiver needs a
  call-site `&mut`; this adds the one, narrowly named exception for the
  compiler-builtin `Push<T>::push` method.
- `differential-execution`: the maintained-corpus Requirement's named
  feature-family list is extended to include array construction via
  `push`, including a capacity-growth and an owned-element case.

## Impact

- `src/ast.rs`, `src/typecheck.rs`, `src/hir.rs`: `Push<T>` trait
  declaration, compiler-synthesized generic impl registration, and the
  receiver-modifier exception for its `push` method.
- `src/ownership.rs`: move-once semantics for `push`'s second argument and
  the exclusive-borrow effect on its receiver.
- `src/eval.rs`: interpreter execution of `push` as a `Vec::push`.
- `src/wasm.rs`, `src/wasm_data.rs`, `src/wasm_runtime.rs`: Core Wasm
  codegen for capacity-doubling growth, realloc-and-copy, and OOM trap
  reuse.
- `src/differential.rs`: new maintained-corpus fixture(s) for `push`,
  including a non-`Copy` element and a capacity-growth boundary.
- `openspec/specs/array-push/spec.md` (new), `openspec/specs/method-call-
  type-checking/spec.md` (modified), `openspec/specs/differential-
  execution/spec.md` (modified).
- `ROADMAP.md`: MAP-075 status `in-progress` → `done`.
- No public Rhodolite ABI or CLI surface change; no `len()`/`xs[i]` syntax
  added.
