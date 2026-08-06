## MODIFIED Requirements

### Requirement: A generic impl's target must name a declared struct or an array of the impl's own type parameter
The system SHALL require a generic `impl`'s target type, after substituting
the impl's own type parameters, to either name a declared struct, or to be
an array whose element type is exactly one of the impl's own type
parameters (the `impl<T> Trait<T> for [T]` shape). Any other target shape
— a concrete-element array, a callable type, a builtin, or any other
non-struct, non-`[T]` shape — SHALL be rejected using the same requirement
and diagnostic a non-generic `impl`'s target already has.

#### Scenario: A generic impl targeting `[T]` with the impl's own type parameter is accepted
- **WHEN** the source declares a trait `Foo<T>` and `impl<T> Foo<T> for [T] { ... }`,
  where `T` is the impl's own type parameter
- **THEN** compilation accepts the impl's target and proceeds to
  contract-check its methods

#### Scenario: A generic impl targeting a concrete-element array is rejected
- **WHEN** the source declares `impl<T> Foo<T> for [int] { ... }` (a
  concrete element type, not the impl's own type parameter)
- **THEN** compilation fails with a diagnostic reporting that the target is
  not a struct

#### Scenario: A generic impl targeting a callable type is rejected
- **WHEN** the source declares `impl<T> Foo<T> for fn(T -> T) { ... }`
- **THEN** compilation fails with a diagnostic reporting that the target is
  not a struct
