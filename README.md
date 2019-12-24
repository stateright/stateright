# Stateright

[![crates.io](https://img.shields.io/crates/v/stateright.svg)](https://crates.io/crates/stateright)
[![docs.rs](https://docs.rs/stateright/badge.svg)](https://docs.rs/stateright)
[![LICENSE](https://img.shields.io/crates/l/stateright.svg)](https://github.com/stateright/stateright/blob/master/LICENSE)

Stateright is a library for specifying actor systems and validating invariants.
It features an embedded [model checker](https://en.wikipedia.org/wiki/Model_checking)
that can verify both abstract models and real systems.

## Examples

As a simple example of an abstract model, we can indicate how to play a
[sliding puzzle](https://en.wikipedia.org/wiki/Sliding_puzzle) game.
Running the model checker against a false invariant indicating that the game is
unsolvable results in the discovery of a counterexample: a sequence of steps
that does solve the game.

```rust
use stateright::*;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Slide { Down, Up, Right, Left }

let puzzle = QuickMachine {
    init_states: || vec![vec![1, 4, 2,
                              3, 5, 8,
                              6, 7, 0]],
    actions: |_, actions| {
        actions.append(&mut vec![
            Slide::Down, Slide::Up, Slide::Right, Slide::Left
        ]);
    },
    next_state: |last_state, action| {
        let empty = last_state.iter().position(|x| *x == 0).unwrap();
        let empty_y = empty / 3;
        let empty_x = empty % 3;
        let maybe_from = match action {
            Slide::Down  if empty_y > 0 => Some(empty - 3), // above
            Slide::Up    if empty_y < 2 => Some(empty + 3), // below
            Slide::Right if empty_x > 0 => Some(empty - 1), // left
            Slide::Left  if empty_x < 2 => Some(empty + 1), // right
            _ => None
        };
        maybe_from.map(|from| {
            let mut next_state = last_state.clone();
            next_state[empty] = last_state[from];
            next_state[from] = 0;
            next_state
        })
    }
};
let solved = vec![0, 1, 2,
                  3, 4, 5,
                  6, 7, 8];
let mut checker = puzzle.checker(|_, state| { state != &solved });
assert_eq!(checker.check(100), CheckResult::Fail { state: solved.clone() });
assert_eq!(checker.path_to(&solved), vec![
    (vec![1, 4, 2,
          3, 5, 8,
          6, 7, 0], Slide::Down),
    (vec![1, 4, 2,
          3, 5, 0,
          6, 7, 8], Slide::Right),
    (vec![1, 4, 2,
          3, 0, 5,
          6, 7, 8], Slide::Down),
    (vec![1, 0, 2,
          3, 4, 5,
          6, 7, 8], Slide::Right)]);
```

See the [examples](https://github.com/stateright/stateright/tree/master/examples)
directory for additional state machines, such as an actor based Single Decree
Paxos cluster and an abstract two phase commit state machine.

To model check, run:

```sh
cargo run --release --example 2pc check 3   # 2 phase commit, 3 resource managers
cargo run --release --example paxos check 2 # paxos, 2 clients
cargo run --release --example wor check 3   # write-once register, 3 clients
```

Stateright also includes a simple runtime for executing an actor state machine
mapping messages to JSON over UDP:

```sh
cargo run --example paxos spawn
```

## Performance

To benchmark model checking speed, run with larger state spaces:

```sh
cargo run --release --example 2pc check 8
cargo run --release --example paxos check 4
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
