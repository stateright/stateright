[![chat](https://img.shields.io/discord/781357978652901386)](https://discord.gg/JbxGSVP4A6)
[![crates.io](https://img.shields.io/crates/v/stateright.svg)](https://crates.io/crates/stateright)
[![docs.rs](https://docs.rs/stateright/badge.svg)](https://docs.rs/stateright)
[![LICENSE](https://img.shields.io/crates/l/stateright.svg)](https://github.com/stateright/stateright/blob/master/LICENSE)

Correctly implementing distributed algorithms such as the
[Paxos](https://en.wikipedia.org/wiki/Paxos_%28computer_science%29) and
[Raft](https://en.wikipedia.org/wiki/Raft_%28computer_science%29) consensus
protocols is notoriously difficult due to inherent nondetermism such as message
reordering by network devices. Stateright is a
[Rust](https://www.rust-lang.org/) actor library that aims to solve this
problem by providing an embedded [model
checker](https://en.wikipedia.org/wiki/Model_checking), a UI for exploring
system behavior ([demo](http://demo.stateright.rs:3000/)), and a lightweight
actor runtime. It also features a linearizability tester that can be run within
the model checker for more exhaustive test coverage than similar solutions such
as [Jepsen](https://jepsen.io/).

![Stateright Explorer screenshot](https://raw.githubusercontent.com/stateright/stateright/master/explorer.png)

## Getting Started

1. **Please see the book, "[Building Distributed Systems With
   Stateright](https://www.stateright.rs)."**
2. A [video
   introduction](https://youtube.com/playlist?list=PLUhyBsVvEJjaF1VpNhLRfIA4E7CFPirmz)
   is also available.
3. Stateright also has detailed [API docs](https://docs.rs/stateright/).
4. Consider also joining the [Stateright Discord
   server](https://discord.gg/JbxGSVP4A6) for Q&A or other feedback.

## Examples

Stateright includes a variety of
[examples](https://github.com/stateright/stateright/tree/master/examples), such
as a [Single Decree Paxos
cluster](https://github.com/stateright/stateright/blob/master/examples/paxos.rs)
and an [abstract two phase commit
model](https://github.com/stateright/stateright/blob/master/examples/2pc.rs).

Passing a `check` CLI argument causes each example to validate itself using
Stateright's model checker:

```sh
# Two phase commit with 3 resource managers.
cargo run --release --example 2pc check 3
# Paxos cluster with 3 clients.
cargo run --release --example paxos check 3
# Single-copy (unreplicated) register with 3 clients.
cargo run --release --example single-copy-register check 3
# Linearizable distributed register (ABD algorithm) with 2 clients
# assuming ordered channels between actors.
cargo run --release --example linearizable-register check 2 ordered
```

Passing an `explore` CLI argument causes each example to spin up the Stateright
Explorer web UI locally on port 3000, allowing you to browse system behaviors:

```sh
cargo run --release --example 2pc explore
cargo run --release --example paxos explore
cargo run --release --example single-copy-register explore
cargo run --release --example linearizable-register explore
```

Passing a `spawn` CLI argument to the examples leveraging the actor
functionality will cause each to spawn actors using the included runtime,
transmitting JSON messages over UDP:

```sh
cargo run --release --example paxos spawn
cargo run --release --example single-copy-register spawn
cargo run --release --example linearizable-register spawn
```

# Features

Stateright contains a general purpose model checker offering:

- Invariant checks via "always" properties.
- Nontriviality checks via "sometimes" properties.
- Liveness checks via "eventually" properties (experimental/incomplete).
- A web browser UI for interactively exploring state space.
- [Linearizability](https://en.wikipedia.org/wiki/Linearizability)
  and [sequential consistency](https://en.wikipedia.org/wiki/Sequential_consistency)
  testers.
- Support for symmetry reduction to reduce state spaces.

Stateright's actor system features include:

- An actor runtime that can execute actors outside the model checker in the
  "real world."
- A model for lossy/lossless duplicating/non-duplicating networks with the
  ability to capture actor message
  [history](https://lamport.azurewebsites.net/tla/auxiliary/auxiliary.html) to
  check an actor system against an expected consistency model.
- Pluggable network semantics for model checking, allowing you to choose
  between fewer assumptions (e.g. "lossy unordered duplicating") or more
  assumptions (speeding up model checking; e.g. "lossless ordered").
- An optional network adapter that provides a lossless non-duplicating ordered
  virtual channel for messages between a pair of actors.

In contrast with other actor libraries, Stateright enables you to [formally
verify](https://en.wikipedia.org/wiki/Formal_verification) the correctness of
your implementation, and in contrast with model checkers such as TLC for
[TLA+](https://lamport.azurewebsites.net/tla/tla.html), systems implemented
using Stateright can also be run on a real network without being reimplemented
in a different language.

## Contribution

Contributions are welcome! Please [fork the
library](https://github.com/stateright/stateright/fork), push changes to your
fork, and send a [pull
request](https://help.github.com/articles/creating-a-pull-request-from-a-fork/).
All contributions are shared under an MIT license unless explicitly stated
otherwise in the pull request.

## License

Stateright is copyright 2018 Jonathan Nadal and other
[contributors](https://github.com/stateright/stateright/graphs/contributors).
It is made available under the MIT License.

To avoid the need for a Javascript package manager, the Stateright repository
includes code for the following Javascript dependency used by Stateright
Explorer:

- [KnockoutJS](https://knockoutjs.com/) is copyright 2010 Steven Sanderson, the
  Knockout.js team, and other contributors. It is made available under the MIT
  License.
