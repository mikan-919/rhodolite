## MODIFIED Requirements

### Requirement: Ambient requirements flow through indirect calls
The system SHALL infer ambient requirements through a higher-order helper from
the named callback selected at each call site. A helper called with callbacks
that require different slots SHALL be treated as separate call specializations
for analysis and generated-code purposes, without exposing effect variables in
source-level types. This SHALL hold identically when the helper is a concrete
instantiation of a generic function: each distinct combination of solved type
arguments and callback binding is its own physically distinct instantiation
(`generic-function-instantiation`), and the system SHALL infer that
instantiation's ambient requirements from the callback selected for it exactly
as it does for a non-generic helper, including through a chain of nested
generic calls that each forward the same callback.

#### Scenario: Callback requirement reaches the caller
- **WHEN** a callback passed to `apply` calls an ambient `db` slot and `apply`
  invokes that callback
- **THEN** the caller of `apply` is inferred to require `db` unless it provides
  `db` around that call

#### Scenario: Missing provider reports the indirect path
- **WHEN** a production root reaches a callback requirement through an indirect
  call without a matching `with` provision
- **THEN** compilation fails before Wasm generation with a path that includes
  the higher-order helper and selected callback

#### Scenario: Callback specializations remain distinct
- **WHEN** the same helper is called once with a callback requiring `db` and
  once with a callback requiring no ambient slot
- **THEN** the no-slot call does not acquire a hidden `db` argument or a
  provider requirement from the other call

#### Scenario: A generic helper's callback requirement reaches the caller
- **WHEN** the source declares `fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }`
  and a function `fetch(id: int -> User?)` that calls an ambient `db` slot,
  and calls `apply(fetch, 1)`
- **THEN** the caller of `apply(fetch, 1)` is inferred to require `db` unless
  it provides `db` around that call

#### Scenario: A generic helper's callback-free specialization stays clean
- **WHEN** the same generic `apply<T, U>` is called once with a callback that
  requires `db` and once, with the same solved type arguments, with a
  callback that requires no ambient slot
- **THEN** the callback-free instantiation's inferred requirements do not
  include `db`, and it acquires no hidden ambient argument from the other
  instantiation

#### Scenario: A callback forwarded through nested generic calls carries its requirement
- **WHEN** a generic helper `fn relay<T, U>(f: fn(T -> U), x: T -> U) { apply(f, x) }`
  forwards its own callback-typed parameter into another generic call, and is
  called with a callback that requires an ambient slot
- **THEN** the caller of `relay` is inferred to require that slot, through a
  path that names both `relay` and `apply`

#### Scenario: Missing provider through a generic helper reports the generic helper and the callback
- **WHEN** a production root calls a generic instantiation with a callback
  that requires an ambient slot, without a matching `with` provision anywhere
  on the path
- **THEN** compilation fails with a diagnostic whose reachability path names
  both the generic helper and the selected callback, the same shape a
  non-generic helper's missing-provider diagnostic already has

### Requirement: Every reachable specialization is reported, not collapsed by shared display name
The inferred-requirements listing SHALL report one entry per reachable
specialization the analysis actually computed a distinct requirement set for,
even when two specializations render to the same display name because they
are different physical instantiations of the same generic declaration (a
generic instantiation's display name does not encode its solved type
arguments or callback binding). Two specializations that happen to compute
identical requirement sets MAY still be listed as separate entries; no
specialization's requirement set SHALL be silently dropped because a later
specialization shares its display name.

#### Scenario: Two instantiations of the same generic function both appear in the listing
- **WHEN** a generic function is called with two different callbacks — one
  requiring an ambient slot, one requiring none — producing two reachable
  instantiations that share the same declared name
- **THEN** the inferred-requirements listing contains an entry reflecting
  each instantiation's own requirement set; neither entry is missing and
  neither entry shows the other instantiation's requirements
