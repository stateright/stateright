A library for specifying state machines and model checking invariants.

## Example

As a simple example, we can simulate a minimal "clock" that alternates between
two hours: zero and one. Then we can enumerate all possible states verifying
that the time is always within bounds and that a path to the other hour begins
at the `start` hour (a model input) followed by a step for flipping the hour
bit.

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

See the [examples/bench/](https://github.com/stateright/stateright/tree/master/examples/bench)
directory for additional examples, such as an implementation of two phase commit.

## Performance

To benchmark model checking speed, run:

```sh
cargo run --release --example bench 2pc
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
