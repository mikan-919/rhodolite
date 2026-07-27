## 1. Type Facts and Signature Index

- [ ] 1.1 Add `KnownType` with nominal name and optionality, then migrate field and local type facts without changing existing struct/enum diagnostics
- [ ] 1.2 Collect owned top-level function parameter and return types into `Decls`, covering forward and recursive declarations

## 2. Direct Call Checking

- [ ] 2.1 Add unit tests and diagnostics for matching, missing, and excess arguments on direct top-level calls
- [ ] 2.2 Add unit tests and diagnostics for known argument-type mismatches while leaving unknown argument expressions undiagnosed
- [ ] 2.3 Infer declared direct-call return types and verify that bindings and existing enum-field checks consume them

## 3. Function Return Checking

- [ ] 3.1 Add unit tests and checks for known final-expression return mismatches
- [ ] 3.2 Add unit tests and checks for known explicit-return mismatches in nested syntax
- [ ] 3.3 Verify that unannotated functions, bare returns, and unknown returned expressions produce no new type diagnostic

## 4. Integration and Documentation

- [ ] 4.1 Add CLI integration coverage for rejected direct calls and return values
- [ ] 4.2 Run the full Rust test suite and confirm the canonical and missing-handler examples retain their existing behavior
- [ ] 4.3 Update `docs/overview.md` and `src/typecheck.rs` boundary documentation to describe the new guarantees and remaining exclusions
