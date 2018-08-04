# Stateright

[![crates.io](https://img.shields.io/crates/v/stateright.svg)](https://crates.io/crates/stateright)
[![docs.rs](https://docs.rs/stateright/badge.svg)](https://docs.rs/stateright)
[![LICENSE](https://img.shields.io/crates/l/stateright.svg)](https://github.com/stateright/stateright/blob/master/LICENSE)

Stateright is a library for specifying state machines and [model
checking](https://en.wikipedia.org/wiki/Model_checking) invariants. Embedding a
model checker into a general purpose programming language allows consumers to
formally verify product implementations in addition to abstract models.

## Example

As a simple example of an abstract model, we can simulate a minimal "clock"
that alternates between two hours: zero and one. Then we can enumerate all
possible states verifying that the time is always within bounds and that a path
to the other hour begins at the `start` hour (a model input) followed by a step
for flipping the hour bit.

```rust
use stateright::*;

struct BinaryClock { start: u8 }

impl StateMachine for BinaryClock {
    type State = u8;

    fn init(&self, results: &mut StepVec<Self::State>) {
        results.push(("start", self.start));
    }

    fn next(&self, state: &Self::State, results: &mut StepVec<Self::State>) {
        results.push(("flip bit", (1 - *state)));
    }
}

let mut checker = BinaryClock { start: 1 }.checker(
    KeepPaths::Yes,
    |clock, time| 0 <= *time && *time <= 1);
assert_eq!(
    checker.check(100),
    CheckResult::Pass);
assert_eq!(
    checker.path_to(&0),
    Some(vec![("start", 1), ("flip bit", 0)]));
```

## More Examples

See the [examples](https://github.com/stateright/stateright/tree/master/examples)
directory for additional state machines, such as an actor based write-once register
and an abstract two phase commit state machine.

To model check, run:

```sh
cargo run --release --example 2pc 3       # 2 phase commit, 3 resource managers
cargo run --release --example wor check 3 # write-once register, 3 clients
```

Stateright also includes a simple runtime for executing an actor state machine
mapping messages to JSON over UDP:

```sh
cargo run --example wor spawn
```

## Performance

To benchmark model checking speed, run with larger state spaces:

```sh
cargo run --release --example 2pc 8
cargo run --release --example wor check 6
```

## Contributing

1. Clone the repository:
   ```sh
   git clone https://github.com/stateright/stateright.git
   cd stateright
   ```
2. Install the latest version of rust:
   ```sh
   rustup update || (curl https://sh.rustup.rs -sSf | sh)
   ```
3. Run the tests:
   ```sh
   cargo test && cargo test --examples
   ```
4. Review the docs:
   ```sh
   cargo doc --open
   ```
5. Explore the code:
   ```sh
   $EDITOR src/ # src/lib.rs is a good place to start
   ```
6. If you would like to share improvements, please
   [fork the library](https://github.com/stateright/stateright/fork), push changes to your fork,
   and send a [pull request](https://help.github.com/articles/creating-a-pull-request-from-a-fork/).

## License

Copyright 2018 Jonathan Nadal and made available under the MIT License.
