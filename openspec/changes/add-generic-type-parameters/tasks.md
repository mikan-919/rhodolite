## 1. AST and grammar

- [ ] 1.1 Add `TypeParam { name, span }` and `type_params: Vec<TypeParam>` to
      `Sig`, `Item::Trait`, and `Item::Impl` in `src/ast.rs`; widen
      `Item::Impl` to `trait_ref: Option<TraitRef>` (`TraitRef { name, args:
      Vec<Type> }`) and `target: Type`, updating every existing reader of the
      old `trait_name`/`type_name` fields.
- [ ] 1.2 Parse `<T, U>` after the `fn` name (shared by free functions, trait
      methods, and impl methods via `sig()`), after the `trait` name, and
      after the `impl` keyword; parse `impl`'s trait reference with optional
      `<...>` type arguments and its target as a full type annotation.
- [ ] 1.3 Reject a type parameter list on `struct`/`enum` with a
      source-positioned diagnostic naming the declaration kind.
- [ ] 1.4 Update `docs/grammar.md` with the `fn`/`trait`/`impl` type parameter
      syntax and the widened `impl` grammar.
- [ ] 1.5 Add focused parser tests: type parameter parsing and spans on
      `fn`/`trait`/`impl`; `impl<T> Map<T> for [T]` round-tripping through
      `TraitRef`/`target`; rejection on `struct`/`enum`; unchanged parsing of
      every existing non-generic `fn`/`trait`/`impl` fixture.

## 2. Declaration-scoped resolution

- [ ] 2.1 Add `hir::TypeParamId` (via the existing `ids!` macro).
- [ ] 2.2 In `typecheck.rs`, build each generic declaration's type parameter
      scope (own `type_params`, plus an impl method's enclosing `impl`
      `type_params`) and report duplicate names with the offending
      declaration's span, including an impl method redeclaring its enclosing
      impl's parameter name.
- [ ] 2.3 Resolve `Named(name)` types in a generic declaration's params,
      return type, trait reference arguments, and target type against this
      scope first, falling back to the existing struct/enum/builtin lookup
      and "undeclared type" diagnostic for any miss.
- [ ] 2.4 Confirm (with a test) that `module::resolve_type` leaves an
      unresolved bare type parameter name unchanged, so it reaches the scope
      check in 2.3 unmodified.
- [ ] 2.5 Add focused typecheck tests: duplicate type parameter names (own
      list and impl/method collision) with span; a type-parameter-shaped name
      used outside any declaring scope hitting the existing "undeclared type"
      diagnostic at its span; scope layering for `impl<T> ... { fn map<U> }`.

## 3. Generic signature representation and invariant

- [ ] 3.1 Add `hir::GenericType` (`Builtin`, `Named`, `Array`, `Callable`,
      `Param(TypeParamId)`) and a per-declaration record (e.g. `GenericFnId`
      arena entry) holding its ordered `Vec<TypeParamId>` and `GenericType`
      params/return/trait-reference-args/target, populated for every generic
      `fn`, trait method, and impl method.
- [ ] 3.2 Do not register generic declarations in `Ids::fns` /
      `Ids::trait_methods` / `Ids::methods` / `Ids::trait_impls`, and do not
      lower their bodies to checked HIR; confirm requirement analysis,
      ownership checking, the interpreter, and Wasm generation never observe
      a generic declaration.
- [ ] 3.3 Add `GenericType::is_concrete` (and a whole-signature variant)
      returning `false` exactly when a `Param` occurs anywhere in the tree.
- [ ] 3.4 Add a regression test enumerating `hir::TypeKind`'s variants and
      asserting none can represent an unbound type parameter (guards the
      "no type variable in concrete HIR" invariant against future edits).
- [ ] 3.5 Add focused tests: a well-formed generic `fn`/`trait`/`impl`
      produces the expected `GenericType`/`Param` references and no entry in
      `Ids`'s callable/method/trait-impl tables; `is_concrete` on constructed
      `GenericType` values with and without `Param`.

## 4. End-to-end verification

- [ ] 4.1 Add `tests/cli.rs` cases for the duplicate-type-parameter and
      out-of-scope-type-parameter diagnostics' rendered text and span, and for
      the `struct`/`enum` type-parameter-list rejection.
- [ ] 4.2 Run the full existing test suite, `cargo fmt --check`, and
      warnings-as-errors Clippy; confirm no existing AST/HIR/interpreter/Wasm
      test output changes.
- [ ] 4.3 Run the existing differential and Wasm byte-determinism checks to
      confirm this change does not alter any existing fixture's interpreter
      or Wasm behavior (no fixture uses the new syntax yet).
- [ ] 4.4 Commit the verified, test-passing state as a single stable snapshot.
