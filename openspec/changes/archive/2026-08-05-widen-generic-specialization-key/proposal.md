## Why

MAP-020 (`add-generic-function-instantiation`) and MAP-025
(`add-generic-trait-resolution`) cache a generic instantiation by
`(GenericFnId, type arguments)` alone. Both changes documented this as a
deliberate, temporary narrowing of MAP-Q5's decided specialization key
(`generic 宣言 ID、型引数、callback 束縛`), safe only because nothing
downstream yet distinguishes behavior by *which* concrete function value is
bound to a callable-typed parameter. That stops being safe the moment
ambient/effect requirement inference (MAP-050, the next task) needs to
attribute different ambient requirements to `apply(double, x)` versus
`apply(sendEmail, x)` even when both instantiate `apply<int, int>` — sharing
one instantiation between them would make the two calls indistinguishable
downstream, so MAP-050 needs a key that already tells them apart before it
can start.

This change widens the specialization key to include callback binding now,
before MAP-050 needs it, exactly as MAP-020/MAP-025 planned.

## What Changes

- `instantiate()`'s cache and "currently instantiating" recursion stack
  (`src/typecheck.rs`) key on `(GenericFnId, normalized type arguments,
  callback binding)` instead of `(GenericFnId, type arguments)`. Callback
  binding is a per-declared-parameter `Option<hir::CallableId>`, resolved at
  the call site from the already-lowered caller body and the existing
  `hir::callable_of`/`hir::resolve_bindings` machinery (`src/hir.rs`,
  previously only consulted downstream by `requirement.rs`/`ambient_abi.rs`).
- Two calls to the same generic declaration with the same type arguments but
  different concrete functions bound to a callable-typed parameter now
  produce **distinct** physical instantiations; two calls with the same type
  arguments *and* the same callback binding still share one, unchanged from
  today.
- Same-key recursion (identical type arguments and callback binding) still
  reserves its instantiation slot before recursing and shares it, unchanged.
  Polymorphic-recursion detection continues to trigger only on a
  type-argument change within a recursive cycle through one generic
  declaration; callback-binding divergence is not a new diagnostic trigger,
  because this language's existing constraint (a callable value is always
  either a named function or an immutable-local alias, never something
  constructed fresh at runtime) makes a callback binding fixed per call site
  and therefore incapable of diverging across recursion depth.
- No new reachability/dead-code pass is added: instantiations are still
  produced lazily, only for a call site actually reached during
  `check_and_lower`'s single deterministic pass, so the widened key does not
  introduce an eager cross-product of type arguments and callback bindings.

## Capabilities

### New Capabilities
(none)

### Modified Capabilities
- `generic-function-instantiation`: the specialization-key requirement for a
  generic free function's instantiation now includes callback binding, with
  a scenario showing two calls sharing type arguments but differing callback
  bindings produce distinct instantiations; the polymorphic-recursion
  requirement is clarified to key on type-argument divergence only; the
  Non-Goal excluding callback identity from the key is removed (this change
  fulfills it) and a Non-Goal is added explaining why callback-binding
  divergence is not itself a polymorphic-recursion trigger.
- `generic-trait-resolution`: the equivalent specialization-key requirement
  for a resolved generic impl method call gets the same widening, reusing
  the same cache MAP-025 already shares with `generic-function-instantiation`
  (MAP-025 Decision 5); its Non-Goal naming MAP-040 is updated the same way.

## Impact

- `src/typecheck.rs`: `instantiate()`, its `Out.instances`/`Out.building`
  cache and recursion stack, `generic_call()` (to compute the callback-key
  vector for a call site), and `check_body()` (to seed a generic
  instantiation's own callback-typed parameters with the binding it was
  instantiated with, so a nested generic call inside its body can resolve
  callback identity transitively).
- `src/hir.rs`: no new public shape: `Bindings`/`callable_of`/
  `resolve_bindings`/`callee_bindings` are reused as-is, now also consulted
  during typecheck-time monomorphization rather than only by
  `requirement.rs`/`ambient_abi.rs` after checking finishes.
- No change to ownership checking (MAP-030 already established it never
  looks at `CallableOwner` or instantiation identity), the interpreter, or
  Wasm generation — a widened-key instantiation is still an ordinary
  `hir::Callable`, indistinguishable to every downstream pass.
