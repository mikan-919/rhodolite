## ADDED Requirements

### Requirement: Methods declare receiver ownership
Method and trait signatures SHALL distinguish shared `&self`, mutable `&mut
self`, and consuming `self`. Implementations SHALL match the trait receiver mode
exactly. Associated functions SHALL continue to omit a receiver.

#### Scenario: Shared receiver
- **WHEN** a method declares `&self`
- **THEN** an ordinary dot call shared-borrows its receiver

#### Scenario: Mutable receiver
- **WHEN** a method declares `&mut self`
- **THEN** its implementation may mutate the receiver through exclusive access

#### Scenario: Trait receiver mismatch
- **WHEN** a trait declares `&self` and an implementation declares `self` or `&mut self`
- **THEN** conformance checking rejects the implementation

### Requirement: Receiver effects are visible at calls
A bound mutable receiver SHALL be called with receiver modifier `&mut`, and a
bound consuming receiver SHALL be called with `move`. A shared receiver SHALL
auto-borrow without a modifier. The modifier SHALL apply before the receiver's
postfix call chain.

#### Scenario: Mutable method call
- **WHEN** source calls `&mut user.rename(name)` on a mutable owner
- **THEN** the selected `&mut self` method receives an exclusive borrow

#### Scenario: Consuming method call
- **WHEN** source calls `move user.finish()` for a method taking `self`
- **THEN** ownership moves into the method and later use of `user` is rejected

#### Scenario: Missing receiver modifier
- **WHEN** a bound owner calls an `&mut self` or consuming method without its required modifier
- **THEN** checking reports the missing ownership mode at the receiver
