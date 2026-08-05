## 1. Zero-behavior-change plumbing

- [x] 1.1 Add `type_params: &'a TypeParamScope` to `Cx` in `src/typecheck.rs`;
      every existing `check_body` call site passes a shared empty
      `TypeParamScope`. Add the same scope parameter to `report_unknown`,
      short-circuiting to "declared" for a name the scope contains.
- [x] 1.2 Decouple `check_body`'s parameter derivation from `Option<&Sig>`:
      take `params: &[(String, KnownType)]` and `span: Span` directly. Update
      the three existing call sites (free fn, impl method, test) to build
      their parameter list the same way they always computed it inline.
- [x] 1.3 Add a focused test asserting a representative non-generic program's
      diagnostics and dumped HIR are byte-identical before and after 1.1/1.2
      (e.g. re-run an existing `typecheck.rs` fixture and compare).
- [x] 1.4 Add a regression test asserting the lexer rejects `#` as an
      identifier character (guards the synthetic rigid-type-parameter naming
      scheme in section 2 against a future collision).

## 2. Rigid whole-body checking of generic free functions

- [x] 2.1 Add `Ids.generic_fns: BTreeMap<String, hir::GenericFnId>`, populated
      in `collect()` next to the existing `record_generic` call for
      `Item::Fn`.
- [x] 2.2 Add a conversion from `hir::GenericType` to `KnownType` that maps
      `Param(id)` to a synthetic, collision-free name (e.g. `#T<index>` from
      the dense `TypeParamId` index) and every other case 1:1 (`Builtin` →
      spelling, `Struct`/`Enum` → the declaration's canonical name, `Array`/
      `Callable` recurse).
- [x] 2.3 In `check_and_lower`'s second pass, add a sibling arm for
      `Item::Fn { sig, body, .. }` when `is_generic(&[], sig)` is true: look
      up the `GenericDecl` via `Ids.generic_fns`, convert its `params`/`ret`
      via 2.2, zip `sig.params[i].name` with the converted types, and check
      the body with `Target::Discard`.
- [x] 2.4 Add focused `typecheck.rs` tests: `identity<T>(x: T -> T) { x }` and
      `apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }` are accepted with no
      call site present; a generic body with a genuine type error (e.g.
      `fn bad<T>(x: T -> int) { x + 1 }`) is rejected the same way a
      non-generic equivalent would be; two distinct type parameters are not
      interchangeable (e.g. `fn same<T, U>(a: T, b: U -> T) { b }` is
      rejected); a type parameter used as a struct literal, field access, or
      method-call receiver is rejected as an undeclared name; a `let`
      annotation inside a generic body naming an in-scope type parameter is
      accepted (exercises 1.1's `report_unknown` scope).

## 3. Call-site type argument inference

- [x] 3.1 Add `hir::substitute(generic_ty: &GenericType, subst: &BTreeMap<TypeParamId, Type>) -> Type`
      in `src/hir.rs`, mirroring `GenericType::is_concrete`'s shape.
- [x] 3.2 Add a unification function comparing a `GenericType` (the callee's
      declared parameter type) against an already-lowered `hir::Type` (an
      argument's checked, concrete type), extending a
      `BTreeMap<TypeParamId, hir::Type>`, and reporting a conflict when the
      same `TypeParamId` is bound to two different `hir::Type`s.
- [x] 3.3 In `resolve()`, add a branch for `ExprKind::Ident(name)` matching
      `decls.generic_fns` (and not `decls.fns`): `synth` each argument
      independently, lower each argument's checked type via `lower_known`,
      unify against `GenericDecl.params[i]` via 3.2, and check every
      declared type parameter received a binding.
- [x] 3.4 Diagnose an unresolved type parameter (no argument bound it) and a
      conflicting one (two arguments imply different types), each with a
      source-positioned span, the type parameter's name, and (for conflicts)
      both inferred types.
- [x] 3.5 Once solved, substitute each parameter type via 3.1 and pass each
      already-`synth`ed argument through the existing `conform` for its
      substituted expected type; extend `Resolved` with the already-lowered
      argument IDs so `call()` does not re-walk them.
- [x] 3.6 Add focused `typecheck.rs` tests: `identity(5)` and `identity("hi")`
      each infer the expected type argument; `apply(double, 3)` infers both
      type arguments from two different argument positions; `make()` with a
      type parameter no argument mentions is diagnosed as unresolved;
      `pair(1, "x")` with a shared type parameter is diagnosed as
      conflicting, naming both inferred types.

## 4. Monomorphization and the instantiation cache

- [x] 4.1 Add a `Vec<(GenericFnId, Vec<hir::Type>, CallableId)>` cache and a
      `Vec<(GenericFnId, Vec<hir::Type>)>` "currently instantiating" stack,
      threaded through `check_and_lower`'s existing per-run state.
- [x] 4.2 On a solved call (3.3-3.5) with no cache hit: check the stack first
      for polymorphic recursion (same `GenericFnId`, different type
      arguments already on the stack) and diagnose it before proceeding, per
      Decision 5; otherwise allocate the concrete `hir::Callable` shell in
      `lowered.callables`, insert the cache entry and push the stack entry
      before checking the body, check/lower the body with
      `Target::Callable(shell_id)` using the substituted `KnownType`s (this
      time letting `report_unknown`/`lower_known` resolve every leaf for
      real), then pop the stack entry. On a cache hit, reuse the existing
      `CallableId` directly.
- [x] 4.3 Wire the resolved/cached `CallableId` into `CallTarget::Direct`, so
      the calling body's `hir::Call::Direct` is indistinguishable from an
      ordinary direct call.
- [x] 4.4 Add focused `typecheck.rs` tests: two calls to `identity` with the
      same argument type share one `CallableId`; calls with different
      argument types produce distinct `CallableId`s, each with no unbound
      type parameter anywhere in its signature or body (assert via
      `GenericType`-style concreteness, or by inspecting the produced
      `hir::Callable`/`hir::Body` directly); a self-recursive generic
      function called with a consistent type argument on every recursive
      call compiles to exactly one instantiation and terminates; a generic
      function that recurses into itself with a different type argument is
      rejected with a polymorphic-recursion diagnostic naming both type
      arguments.

## 5. End-to-end verification

- [x] 5.1 Add `tests/cli.rs` success cases: a program calling `identity` at
      two different concrete types and a program calling `apply` with a
      named function callback, each run to completion via the ordinary CLI
      interpreter path with the expected printed result.
- [x] 5.2 Add `tests/cli.rs` failure cases: unresolved type argument,
      conflicting type argument, and polymorphic recursion, each asserting
      the rendered diagnostic text and span.
- [x] 5.3 Add an ownership-focused test exercising `apply`'s consuming
      callback with a non-`Copy` argument, confirming ownership checking
      plans exactly one move using its existing (unmodified) rule for a
      non-`Copy` value passed to a function parameter.
- [x] 5.4 Add a differential/Wasm test building the `identity`/`apply`
      program from 5.1 for both the interpreter and Wasm target, confirming
      matching results and byte-identical Wasm across two consecutive builds
      of the same source.
- [x] 5.5 Run the full existing test suite, `cargo fmt --check`, and
      warnings-as-errors Clippy; confirm no existing AST/HIR/interpreter/Wasm
      test output changes for any program that declares no generic function
      call.
- [x] 5.6 Sync `generic-type-parameters`'s delta spec in this change into
      `openspec/specs/generic-type-parameters/spec.md` and commit the
      verified, test-passing state as a single stable snapshot.
