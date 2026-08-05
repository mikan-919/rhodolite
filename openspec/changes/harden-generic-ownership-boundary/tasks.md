## 1. Copy / non-Copy classification across instantiations

- [ ] 1.1 Add a test with a generic free function (e.g. `identity<T>`) called
      once at a `Copy` type argument (`int`) and once at a non-`Copy` type
      argument (a struct) in the same program; assert the ownership plan
      classifies each instantiation's parameter independently (no move
      needed for the `Copy` call, a move planned for the non-`Copy` call).
- [ ] 1.2 Add the same scenario for a generic impl method (an `impl<T>` on a
      struct with a method taking a `T`-typed parameter or consuming `self`
      at `T`), called at a `Copy` and a non-`Copy` type argument.

## 2. Move-after-use rejection

- [ ] 2.1 Add a test where a generic free function instantiated at a
      non-`Copy` type argument is called twice with the same binding without
      `move`/`clone()`; assert compilation is rejected with the same
      diagnostic shape a non-generic call would produce.
- [ ] 2.2 Add the same scenario for a generic impl method's consuming `self`
      or non-`Copy` parameter.

## 3. Drop planning

- [ ] 3.1 Add a test where a generic free function instantiated at a
      non-`Copy` type argument binds a value it neither returns nor
      otherwise consumes; assert the ownership plan includes a drop for that
      binding at the end of its scope.
- [ ] 3.2 Add the same scenario for a generic impl method's body.

## 4. Consuming callback moves exactly once

- [ ] 4.1 Add the generic impl method counterpart of the existing
      `汎用関数越しの非copy引数はちょうど1度moveされる` test (the
      free-function version already exists from MAP-020): a generic impl
      method that passes a non-`Copy` value into a consuming callback
      parameter; assert exactly one move at the call site and exactly one
      move inside the instantiated body.

## 5. Type-variable-leak regression test

- [ ] 5.1 Add a test compiling a program with at least two distinct
      instantiations of the same generic declaration using
      differently-named type parameters (e.g. both `T` and `U`); assert the
      dumped checked HIR / ownership plan contains no `#`-prefixed
      rigid-check placeholder name.

## 6. Fix any discovered gap

- [ ] 6.1 If any test added in sections 1-5 fails against current
      `src/ownership.rs`/`src/typecheck.rs`, fix the gap (do not weaken or
      skip the test) before proceeding.

## 7. Verification and snapshot

- [ ] 7.1 Run the full test suite (`cargo test`) and confirm all new and
      existing tests pass.
- [ ] 7.2 Run `cargo fmt --check` and Clippy with warnings denied
      (`cargo clippy --all-targets -- -D warnings`); fix any findings.
- [ ] 7.3 Commit the verified, passing state as a single snapshot per the
      repository's git-snapshot convention.
