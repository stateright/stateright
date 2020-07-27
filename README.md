[![crates.io](https://img.shields.io/crates/v/stateright.svg)](https://crates.io/crates/stateright)
[![docs.rs](https://docs.rs/stateright/badge.svg)](https://docs.rs/stateright)
[![LICENSE](https://img.shields.io/crates/l/stateright.svg)](https://github.com/stateright/stateright/blob/master/LICENSE)

Correctly implementing distributed algorithms such as the
[Paxos](https://en.wikipedia.org/wiki/Paxos_%28computer_science%29) and
[Raft](https://en.wikipedia.org/wiki/Raft_%28computer_science%29) consensus
protocols is notoriously difficult due to the presence of nondeterminism,
whereby nodes lack perfectly synchronized clocks and the network reorders and
drops messages.  Stateright is a library and tool for designing, implementing,
and verifying the correctness of distributed systems by leveraging a technique
called [model checking](https://en.wikipedia.org/wiki/Model_checking).  Unlike
with traditional model checkers, systems implemented using Stateright can also
be run on a real network without being reimplemented in a different language.
It also features a web browser UI that can be used to interactively explore how
a system behaves, which is useful for both learning and debugging.

![Stateright Explorer screenshot](https://raw.githubusercontent.com/stateright/stateright/master/explorer.jpg)

A typical workflow might involve:

```sh
# 1. Interactively explore reachable states in a web browser.
cargo run --release --example paxos explore 2 1 localhost:3000

# 2. Then model check to ensure all edge cases are addressed.
cargo run --release --example paxos check 4 2

# 3. Finally, run the service on a real network, with JSON messages over UDP...
cargo run --release --example paxos spawn

# ... and send it commands. In this example 1 and 2 are request IDs, while "X"
#     is a value.
nc -u localhost 3000
{"Put":[1,"X"]}
{"Get":2}
```

Checker features include:

- Invariants via "always" properties.
- Nontriviality checks via "sometimes" properties.
- Liveness checks via "eventually" properties (with some limitations at this time).
- UI for interactively exploring state space.

Actor system features include:

- Ability to execute actors via JSON over UDP.
- Can model lossy/lossless networks.
- Can model duplicating/non-duplicating networks.
- Can capture actor system
  [history](https://lamport.azurewebsites.net/tla/auxiliary/auxiliary.html)
  to check properties such as
  [linearizability](https://en.wikipedia.org/wiki/Linearizability).

## Examples

Stateright includes a variety of
[examples](https://github.com/stateright/stateright/tree/master/examples), such
as an actor based Single Decree Paxos cluster and an abstract two phase commit
model.

To model check, run:

```sh
# Two phase commit with 3 resource managers.
cargo run --release --example 2pc check 3
# Paxos cluster (fixed size of 3) with 4 puts and 2 gets, all concurrent.
cargo run --release --example paxos check 4 2
# Single-copy register with 4 puts and 2 gets, all concurrent.
cargo run --release --example single-copy-register 4 2
```

To interactively explore a model's state space in a web browser UI, run:

```sh
cargo run --release --example 2pc explore
cargo run --release --example paxos explore
cargo run --release --example single-copy-register explore
```

Stateright also includes a simple runtime for executing an actor mapping
messages to JSON over UDP:

```sh
cargo run --release --example paxos spawn
cargo run --release --example single-copy-register spawn
```

## Performance

To benchmark model checking speed, run with larger state spaces:

```sh
cargo run --release --example 2pc check 9
cargo run --release --example paxos check 6 2
cargo run --release --example single-copy-register check 3 3
```

A script that runs all the examples multiple times is provided for convenience:

```sh
./bench.sh
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

Stateright is copyright 2018 Jonathan Nadal and other contributors. It is made
available under the MIT License.

To avoid the need for a Javascript package manager, the Stateright repository
includes code for the following Javascript dependencies used by Stateright
Explorer:

- [KnockoutJS](https://knockoutjs.com/) is copyright 2010 Steven Sanderson, the
  Knockout.js team, and other contributors. It is made available under the MIT
  License.
