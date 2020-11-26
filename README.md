[![crates.io](https://img.shields.io/crates/v/stateright.svg)](https://crates.io/crates/stateright)
[![docs.rs](https://docs.rs/stateright/badge.svg)](https://docs.rs/stateright)
[![LICENSE](https://img.shields.io/crates/l/stateright.svg)](https://github.com/stateright/stateright/blob/master/LICENSE)

Correctly designing and implementing distributed algorithms such as the
[Paxos](https://en.wikipedia.org/wiki/Paxos_%28computer_science%29) and
[Raft](https://en.wikipedia.org/wiki/Raft_%28computer_science%29) consensus
protocols is notoriously difficult due to the presence of inherent
nondeterminism, whereby nodes lack synchronized clocks and IP networks can
reorder, drop, or redeliver messages. Stateright is an actor library for
designing, implementing, and verifying the correctness of such distributed
systems using the [Rust programming language](https://www.rust-lang.org/). It
leverages a verification technique called [model
checking](https://en.wikipedia.org/wiki/Model_checking), a category of property
based testing that involves enumerating every possible outcome of a
nondeterministic system rather than randomly testing a subset of outcomes.

Stateright's model checking features include:

- Invariants via "always" properties.
- Nontriviality checks via "sometimes" properties.
- Liveness checks via "eventually" properties (with some limitations at this
  time).
- A web browser UI for interactively exploring state space.
- [Linearizability](https://en.wikipedia.org/wiki/Linearizability)
  and [sequential consistency](https://en.wikipedia.org/wiki/Sequential_consistency)
  testers for concurrent systems (such as actors operating on an asynchronous
  network).

Stateright's actor system features include:

- The ability to execute actors outside the model checker, sending messages over
  UDP.
- A model for lossy/lossless duplicating/non-duplicating networks with the
  ability to capture actor message [history](https://lamport.azurewebsites.net/tla/auxiliary/auxiliary.html)
  to check an actor system against an expected consistency model.
- An optional network adapter that provides a lossless non-duplicating ordered
  virtual channel for messages between a pair of actors.

In contrast with other actor libraries, Stateright enables you to [formally
verify](https://en.wikipedia.org/wiki/Formal_verification) the correctness of
both your design and implementation, which is particularly useful for
distributed algorithms.

In contrast with most other model checkers (like TLC for
[TLA+](https://lamport.azurewebsites.net/tla/tla.html)), systems implemented
using Stateright can also be run on a real network without being reimplemented
in a different language, an idea pioneered by model checkers such as
[VeriSoft](https://citeseerx.ist.psu.edu/viewdoc/summary?doi=10.1.1.25.8581)
and [CMC](https://www.microsoft.com/en-us/research/publication/cmc-a-pragmatic-approach-to-model-checking-real-code/).
Stateright also features a web browser UI that can be
used to interactively explore how a system behaves, which is useful for both
learning and debugging. See [demo.stateright.rs](http://demo.stateright.rs:3000/)
to explore an abstract model for [two phase
commit](https://en.wikipedia.org/wiki/Two-phase_commit_protocol).

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

## Getting Started

A short book is in the works: "[Building Distributed Systems With
Stateright](https://www.stateright.rs)."

## Community

Join the [Stateright Discord server](https://discord.com/channels/781357978652901386).

## Examples

Stateright includes a variety of
[examples](https://github.com/stateright/stateright/tree/master/examples), such
as an actor based [Single Decree Paxos
cluster](https://github.com/stateright/stateright/blob/master/examples/paxos.rs)
and an [abstract two phase commit
model](https://github.com/stateright/stateright/blob/master/examples/2pc.rs).

To model check, run:

```sh
# Two phase commit with 3 resource managers.
cargo run --release --example 2pc check 3
# Paxos cluster with 3 clients.
cargo run --release --example paxos check 3
# Single-copy register with 3 clients.
cargo run --release --example single-copy-register check 3
# Linearizable distributed register with 3 clients.
cargo run --release --example linearizable-register check 3
```

To interactively explore a model's state space in a web browser UI, run:

```sh
cargo run --release --example 2pc explore
cargo run --release --example paxos explore
cargo run --release --example single-copy-register explore
cargo run --release --example linearizable-register explore
```

Stateright also includes a simple runtime for running actors, with messages
transmitted via UDP. The included examples use JSON for message
serialization/deserialization, but Stateright can handle messages in any
format.

```sh
cargo run --release --example paxos spawn
cargo run --release --example single-copy-register spawn
cargo run --release --example linearizable-register spawn
```

## Model Checking Performance

Model checking is computationally expensive, so Stateright features a
variety of optimizations to help minimize model checking time. To
benchmark model checking performance, run with larger state spaces.

The repository includes a script that runs all the examples multiple times,
which is particularly useful for validating that changes do not introduce
performance regressions.

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
