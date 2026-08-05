## 1. HIR representation additions

- [ ] 1.1 Add `type_params: Vec<hir::TypeParamId>` to `hir::TraitDecl`
      (empty for a non-generic trait), populated in `collect()`'s
      `Item::Trait` branch from the same `declare_type_params` call already
      made there.
- [ ] 1.2 Add `hir::substitute_generic(ty: &GenericType, subst:
      &BTreeMap<TypeParamId, GenericType>) -> GenericType` in `src/hir.rs`,
      mirroring `substitute`'s shape but leaving an unbound `Param` as
      itself instead of requiring a total binding.
- [ ] 1.3 Add focused `hir.rs` tests for `substitute_generic`: a bound
      `Param` is replaced, an unbound `Param` passes through unchanged, and
      nested `Array`/`Callable` shapes recurse correctly — mirroring the
      existing `substitute`/`is_concrete` test style.

## 2. Generic impl trait-reference and target validation

- [ ] 2.1 In `collect()`'s `Item::Impl` branch, when a generic impl's
      `trait_ref.name` does not resolve against `nominal.traits`, push a
      "`{name}` は trait ではありません" diagnostic at the impl's span
      (replacing today's silent `trait_: None`).
- [ ] 2.2 When it does resolve, compare `trait_ref.args.len()` against the
      resolved trait's `type_params.len()` (task 1.1); on a mismatch, push
      a diagnostic naming the trait and both counts.
- [ ] 2.3 Reject a generic impl whose target's outer shape (after
      substituting the impl's own type parameters) is not `Struct` — reuse
      `check_impl`'s "`{type}` は struct ではありません" wording — for
      `Array`, `Callable`, `Builtin`, and a bare `Param` (blanket impl)
      target shapes.
- [ ] 2.4 Add a `(struct name, trait name) -> Span` tracking table
      alongside `Ids.trait_impls`; when a generic-touching impl declares a
      pair already present (from either a prior generic-touching impl or an
      existing non-generic `Ids.trait_impls` entry), push a "`{trait}` は
      `{type}` に対して既に実装されています" diagnostic naming both spans.
- [ ] 2.5 Add focused `typecheck.rs` tests for each of 2.1-2.4: unknown
      trait name, arity mismatch, non-struct target (array and blanket
      cases), and a duplicate `(struct, trait)` pair (generic-vs-generic and
      generic-vs-non-generic).

## 3. Contract-checking a generic impl against its trait

- [ ] 3.1 Add a function building, for one trait, a uniform contract table
      over both its concrete `TraitMethodDecl`s and its generic `GenericDecl`
      entries (`GenericOwner::Trait`), each as `(trait-level type params,
      method's own type params, receiver, params: Vec<GenericType>, ret:
      GenericType)` — wrapping a concrete method's `hir::Type`s in a
      `GenericType` with no `Param` so both cases share one comparison path.
- [ ] 3.2 Add trait-level substitution: build `subst_trait` from the trait's
      own type params (1.1) mapped to the impl's `trait_args`, and apply
      `substitute_generic` (1.2) to the contract method's receiver/params/
      ret.
- [ ] 3.3 Add method-level positional correspondence: zip the trait method's
      own type params with the impl-provided method's own type params (a
      count mismatch is itself a signature-mismatch diagnostic), build a
      `Param`-to-`Param` renaming, and apply it to the trait-level-
      substituted signature from 3.2.
- [ ] 3.4 Compare the fully impl-scoped expected signature against the
      impl-provided method's actual `GenericDecl` (receiver/params/ret) by
      `GenericType`/`Option<ReceiverMode>` equality; reuse `check_impl`'s
      three mismatch messages (receiver/params/ret), generalized to
      `GenericType`.
- [ ] 3.5 Report a trait-declared method the impl never provides ("missing"),
      and a method name the impl declares twice ("duplicated"), reusing
      `check_impl`'s existing wording.
- [ ] 3.6 Wire 3.1-3.5 into `collect()`'s (or a dedicated post-declaration)
      pass for every `Item::Impl` with a resolved, arity-correct,
      struct-targeted trait reference (tasks 2.1-2.3), independent of
      whether any call site exists.
- [ ] 3.7 Add focused `typecheck.rs` tests: a matching generic impl is
      accepted; a missing method, a duplicated method, a parameter-type
      mismatch, a return-type mismatch, a receiver-mode mismatch, and an own
      type-parameter count mismatch are each rejected with the expected
      message.

## 4. Rigid whole-body checking of generic impl methods

- [ ] 4.1 Replace body-checking's `continue` over a generic impl method
      (the `Item::Impl` arm of `check_and_lower`'s main loop) with a call
      into a generalized rigid-checking function reusing MAP-020's
      `check_rigid`/`known_generic`/`rigid_name` machinery, keyed by the
      method's `GenericFnId` (via a new `Ids.generic_impl_methods` index
      populated next to `record_generic`'s existing call for `Item::Impl`,
      mirroring `Ids.generic_fns`).
- [ ] 4.2 Build the method's receiver binding as the ordinary, concrete
      `receiver_type(mode, struct_name)` (Decision 1 in design.md guarantees
      the impl's target is already a concrete struct), not a synthetic name.
- [ ] 4.3 Add focused `typecheck.rs` tests mirroring MAP-020's generic-body
      test set: a well-formed generic impl method body is accepted without
      being called; a genuine type error in an uncalled generic impl method
      body is reported; two of the impl's own type parameters (or one of the
      impl's and one of the method's own) are not interchangeable.

## 5. Method-call resolution fallback to generic impls

- [ ] 5.1 Add `Ids.generic_impls: BTreeMap<(String struct name, String
      method name), Vec<hir::GenericFnId>>`, populated next to
      `record_generic`'s existing call for `Item::Impl` (Decision 1
      guarantees `target` is always a concrete `Struct`, so this is a plain
      equality-keyed index, no unification needed to populate it).
- [ ] 5.2 In `resolve()`'s `ExprKind::Field` arm, after today's existing
      `from_type`/`concrete_target` lookup returns no match, look up
      candidates in `Ids.generic_impls` keyed by the receiver's resolved
      struct name and the called method name.
- [ ] 5.3 Zero candidates: fall through to the existing "呼び出し先が決まり
      ません" diagnostic unchanged.
- [ ] 5.4 More than one candidate: diagnose ambiguity, naming the receiver's
      type, the method name, and each candidate's trait name and
      declaration span, before inferring anything.
- [ ] 5.5 Exactly one candidate: `synth` each call argument and `unify()`
      them against the candidate's `GenericDecl.params`, reusing MAP-020's
      `generic_call` unification loop and its "unresolved"/"conflicting"
      diagnostics; require every one of the candidate's `type_params` to be
      bound (Decision 1: none come from the receiver).
- [ ] 5.6 Add focused `typecheck.rs` tests: a single-candidate call infers
      its type argument and resolves; a zero-candidate call produces the
      existing "no match" diagnostic unchanged; a two-candidate call is
      diagnosed as ambiguous naming both traits; an existing non-generic
      method call's resolution and diagnostics are byte-identical before and
      after this change.

## 6. Monomorphization

- [ ] 6.1 Add a `Vec<((hir::StructId, hir::TraitId), hir::TraitImplId)>`
      cache for lazily allocating one `TraitImplDecl` per `(struct, trait)`
      pair the first time any of its methods is resolved, alongside
      `check_and_lower`'s existing per-run state.
- [ ] 6.2 On a solved call (5.5) with no existing instantiation-cache entry:
      look up or allocate the `TraitImplDecl` (6.1, `methods` left empty —
      see design.md Decision 5), then reuse MAP-020's `instantiate()`
      shell-first/cache/recursion-stack flow unchanged except for the
      shell's `owner: CallableOwner::TraitImpl(id)` instead of
      `CallableOwner::Free`.
- [ ] 6.3 Wire the resolved/cached `CallableId` into `CallTarget::Method
      { callable, recv }`, applying `conform_receiver` unchanged.
- [ ] 6.4 Add focused `typecheck.rs` tests: two calls to the same generic
      impl method with the same argument type share one `CallableId`; calls
      with different argument types produce distinct `CallableId`s, each
      concrete; a self-recursive generic impl method called with a
      consistent type argument compiles to one instantiation; a generic
      impl method recursing into itself with a different type argument is
      rejected with a polymorphic-recursion diagnostic; a free-function and
      an impl-method instantiation with the same type arguments do not share
      a cache entry.

## 7. End-to-end verification

- [ ] 7.1 Add `tests/cli.rs` diagnostic tests: unknown trait reference,
      trait-reference arity mismatch, non-struct generic impl target,
      duplicate `(struct, trait)` impl pair, missing/duplicated/mismatched
      contract method, ambiguous method-call resolution, unresolved type
      argument, conflicting type argument, and polymorphic recursion — each
      asserting rendered diagnostic text and span.
- [ ] 7.2 Add a `tests/cli.rs` success case: a program declaring a
      struct-targeted generic trait/impl (per design.md Decision 1) and a
      method call that resolves and runs to completion via the ordinary CLI
      interpreter path with the expected printed result.
- [ ] 7.3 Add a differential/Wasm test building the program from 7.2 for
      both the interpreter and Wasm target, confirming matching results and
      byte-identical Wasm across two consecutive builds of the same source.
- [ ] 7.4 Run the full existing test suite, `cargo fmt --check`, and
      warnings-as-errors Clippy; confirm no existing AST/HIR/interpreter/Wasm
      test output changes for any program that declares no generic trait or
      impl, and that every existing non-generic trait/impl test still passes
      unchanged.
- [ ] 7.5 Sync `generic-type-parameters`'s delta spec in this change into
      `openspec/specs/generic-type-parameters/spec.md` and commit the
      verified, test-passing state as a single stable snapshot.
