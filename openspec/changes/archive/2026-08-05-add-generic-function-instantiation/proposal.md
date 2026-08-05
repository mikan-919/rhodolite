## Why

MAP-010 gave `fn`/`trait`/`impl` type parameter syntax and a `GenericType`
representation, but a generic declaration's body is never checked and a call
to it never resolves — `identity`/`apply`-shaped functions parse but cannot be
used. MAP-020 is the next step on the roadmap to generic `map`: it must make a
generic function's body provably well-typed on its own, and let a concrete
call site drive inference and produce a runnable, fully concrete instantiation
— without yet touching generic trait/impl resolution (MAP-025), widening the
whole-program specialization key with callback identity (MAP-040), or
ambient-requirement inference through generic callbacks (MAP-050).

## What Changes

- Type-check every generic free function's body once, treating its own type
  parameters as rigid, mutually-distinct opaque types — independent of
  whether the function is ever called, so a bad generic body is reported even
  if unused.
- At a call site whose callee is a generic free function, infer every type
  argument by structurally matching each argument's checked type against the
  callee's generic parameter types. No syntax for explicit type arguments is
  added (consistent with MAP-Q2); an unresolved type parameter (no argument
  constrains it) or a conflicting one (two arguments imply different concrete
  types for the same parameter) is diagnosed before execution.
- Given a fully solved substitution, build a concrete, monomorphized
  `hir::Callable` for that `(generic declaration, type arguments)` pair by
  substituting type parameters through the declaration's signature and
  re-checking/lowering its body against the concrete types. Instantiations
  are cached by declaration and type arguments so repeated calls with the
  same type arguments share one instance, and a self-recursive generic call
  with the same type arguments reuses an already-reserved instance instead of
  recursing forever.
- Diagnose polymorphic recursion — a generic function's own body recursively
  calling itself with different type arguments — before attempting to build
  an unbounded chain of instantiations. Cross-declaration recursion cycles
  and the full whole-program specialization key (callback identity, trait
  impl choice, reachability-based generation) are explicitly out of scope;
  MAP-040 owns widening this.
- A concrete instantiation is an ordinary `hir::Callable` reachable through
  an ordinary `hir::Call::Direct`, so ownership checking, requirement
  analysis, the interpreter, and Wasm generation run over it unmodified —
  this change adds no new `hir::Call`/`hir::Type` variant and no new
  execution-path code in those passes.
- Generic trait methods and generic impl methods (MAP-025), and any use of a
  generic declaration from inside `trait`/`impl`, are unaffected and remain
  excluded from checking/execution exactly as MAP-010 left them.

## Capabilities

### New Capabilities
- `generic-function-instantiation`: rigid whole-body checking of a generic
  free function, call-site type argument inference (success and diagnosed
  failure), and call-site monomorphization into cached, concrete, executable
  `hir::Callable` instances, including the self-recursion sharing rule and
  the polymorphic-recursion diagnostic.

### Modified Capabilities
- `generic-type-parameters`: MAP-010's requirement that "a generic
  declaration does not reach execution" only continues to hold for a generic
  free function that is never called. A called generic free function is now
  monomorphized into concrete HIR that ownership checking, requirement
  analysis, the interpreter, and Wasm generation do observe. Generic trait
  methods and generic impl methods are unaffected and still never reach
  execution (that boundary moves with MAP-025).

## Impact

- `src/typecheck.rs`: add rigid whole-body checking for generic free
  functions (reusing the existing single-pass expression checker with type
  parameters standing in as opaque, self-distinct types); add call resolution
  for a generic callee (argument-driven unification, unresolved/conflicting
  diagnostics); add substitution and on-demand, cached, recursion-safe
  monomorphization into `hir::Callable`.
- `src/hir.rs`: add a substitution helper from `GenericType` + a type-argument
  assignment to a concrete `hir::Type` (no new `Type`/`TypeKind` variant).
- `src/ownership.rs`, `src/eval.rs`, `src/wasm.rs`, `src/requirement.rs`,
  `src/ambient_abi.rs`: unchanged — they already process whatever is in
  `hir::Program`'s ordinary arenas generically.
- No new CLI command, public ABI, or external dependency. Differential and
  Wasm byte-determinism checks extend to cover programs that call a generic
  free function.
