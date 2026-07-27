## 1. Array Type Representation and Syntax

- [ ] 1.1 Add parser tests for `[T]`, `[T]?`, `[T?]`, and nested array types
- [ ] 1.2 Replace the flat AST type representation with recursive named and array type shapes while preserving outer optionality
- [ ] 1.3 Parse array type annotations in every existing type position and keep existing named-type behavior green
- [ ] 1.4 Resolve named leaf types recursively inside array types across module boundaries, with module tests

## 2. Array Literal Type Checking

- [ ] 2.1 Extend the type checker's known-type representation, formatting, equality, and outer optional injection to recursive array types
- [ ] 2.2 Infer homogeneous non-empty array literal types and diagnose conflicting known element types
- [ ] 2.3 Check array literal elements against an expected element type, including empty arrays, `nil`, nested arrays, and optional element injection
- [ ] 2.4 Keep array values invariant in their element types while allowing existing optional injection on the outer array

## 3. For-Loop Type Checking

- [ ] 3.1 Require known `for` iterables to be non-optional arrays and diagnose known non-array and optional-array iterables
- [ ] 3.2 Bind the array element type to the loop variable and verify existing field and expression checks run inside the loop body
- [ ] 3.3 Preserve unknown-iterable behavior without inferring a loop variable type from later uses

## 4. Canonical Program and Documentation

- [ ] 4.1 Replace the canonical program's undeclared `Users` annotation with `[User]` and add an integration assertion that its loop is statically checked
- [ ] 4.2 Document array type syntax, literal inference, empty-array context, invariance, and `for` typing in the grammar and project overview
- [ ] 4.3 Run formatting, the full Rust test suite, the canonical main/test flows, and strict OpenSpec validation
