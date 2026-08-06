## MODIFIED Requirements

### Requirement: Receiver effects are visible at calls
A bound mutable receiver SHALL be called with receiver modifier `&mut`, and a
bound consuming receiver SHALL be called with `move`. A shared receiver SHALL
auto-borrow without a modifier. The modifier SHALL apply before the receiver's
postfix call chain. The compiler-builtin `Push<T>::push` method is the one
named exception to the mutable-receiver rule: a call to it auto-borrows its
array receiver exclusively without a call-site `&mut`, matching the implicit
borrow already used for an ambient-slot `&mut self` call such as
`db.save(...)`.

#### Scenario: Mutable method call
- **WHEN** source calls `&mut user.rename(name)` on a mutable owner
- **THEN** the selected `&mut self` method receives an exclusive borrow

#### Scenario: Consuming method call
- **WHEN** source calls `move user.finish()` for a method taking `self`
- **THEN** ownership moves into the method and later use of `user` is rejected

#### Scenario: Missing receiver modifier
- **WHEN** a bound owner calls an `&mut self` or consuming method without its required modifier
- **THEN** checking reports the missing ownership mode at the receiver

#### Scenario: `push` needs no receiver modifier
- **WHEN** source calls `xs.push(y)` on a bound array local with no `&mut`
  written before `xs`
- **THEN** checking accepts the call and does not report a missing
  receiver modifier
