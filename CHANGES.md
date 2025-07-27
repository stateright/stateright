# Changes

## 0.31.0

Andrew Jeffery <dev@jeffas.io>

- Simplify how checker waits for shutdown.
- Panic on property name collision.

cjen1 <cjj39@cam.ac.uk>

- Fix bug in eventually property checker.

Jonathan Nadal <jon.nadal@gmail.com>

- Address nondeterministic test.

Liangrun Da <liangrun_da@outlook.com>

- Add Raft protocol example.

Maurice Lam <yukl@google.com>

- Add support for random selection.
- Add an example of a last writer wins register demonstrating random
  selection.

Peiyang He <peiyang_he@smail.nju.edu.cn>

- Adding missing fields to `ActorModelState` equality/serialization
  and correct field name typo.
- Correct typos in docs.
- Add support for actor recovery and storage.
- Fix bug in Explorer.
- Update Explorer to use action index rather than state digests to
  reference a behavior, making the URLs more stable.

## 0.30.2

Andrew Jeffery <dev@jeffas.io>

- Add ability to define when checking finishes via `HasDiscoveries`.
- Add optional checker timeout.

D. Reusche <code@degregat.net>

- Add `interaction.rs` example.

Liangrun Da <liangrun_da@outlook.com>

- Fix handling of non-unique states for unordered duplicating networks.

## 0.30.1

Andrew Jeffery <dev@jeffas.io>

- Cleanly handle panics during model checking.
- Leverage `tiny_http` for Explorer.
- Print fingerprint path for discoveries in default report.
- Add names to actors and show in Explorer.
- Introduce simulation checker.

Jonathan Nadal <jon.nadal@gmail.com>

- Fix unintentional spin-wait loop in real-world runtime (`spawn`).
- Fix nondeterministic test result.

remzi <13716567376yh@gmail.com>

- Simplify `DGraph` (used by tests) by eliminating unnecessary cloning.

## 0.30.0

Notable changes follow, grouped by author.

Andrea Stedile <andrea.stedile@studenti.unitn.it>

- Support crash failures.

Andrew Jeffery <dev@jeffas.io>

- Enhance how properties are displayed in the Explorer UI.
- Introduce an "on-demand" checker for Explorer.
- Introduce depth tracking and max depth checking.
- Introduce named timers.
- Sort discoveries.
- Introduce a `join_and_report` method to reduce time overestimation.

David Rusu <davidrusu.me@gmail.com>

- Fix a bug in the new on-demand checker.

Jonathan Nadal <jon.nadal@gmail.com>

- Address a bug for ordered network checking that would result in not exploring
  the complete state space. Thank you to Andrea Stedile for identifying this
  problem.

Wink Saville <wink@saville.com>

- Improve `tcpdump` usage details in the examples.

## 0.29.0

This release adds support for symmetry reduction, courtesy of Chris Jensen
(`@Cjen1` on GitHub).  It also introduces the ability to choose network
semantics, which can be helpful for reducing the state space. Options are:
ordered, unordered duplicating, and unordered non-duplicating.
`DuplicatingNetwork` was removed in favor of capturing that aspect via the new
`Network` type, enabling the library to leverage the most efficient data
structure for each particular use case.

## 0.28.0

Stateright now distinguishes between the number of states (including
regenerated) and the number of unique states. To facilitate this,
`Checker::generated_count` has been removed in favor of `state_count` and
`unique_state_count` methods.

## 0.27.1

This release introduces the ability to quickly navigate forward/backward along
a path using `down`/`up` or `j`/`k`.

## 0.27.0

This release introduces multiple enhancements for Stateright Explorer. Explorer
now shows ignored actions. It also has labels for previous/next states that
match the current state. Arguably the biggest improvement is that Explorer now
shows all properties (not just properties with discoveries) and explains the
checker outcomes for each property. There are also some minor styling changes
in the UI.

Another small improvement is that `ActorModel` now overrides rendering of the
`Deliver` action with a more concise/intuitive representation.

While preparing an example for a revised screenshot, I accidentally created a
model with unwanted nondeterminism: my model depended on iteration order, which
in turn depended upon a random seed. Root causing took a long time, so I
improved the error message that shows when the checker detects unwanted
nondetermism to make similar mistakes easier to debug in the future.

## 0.26.1

This release introduces a `PathRecorder` visitor and a `VectorClock` utility
type. It also adds an `Actor` implementation for `Vec<(Id, Msg)>` for any
message type `Msg`, which can be used as a simple client actor for exercising a
system.

## 0.26.0

`RegisterActor::Client` now has a `put_count` field that indicates how many
`RegisterMsg::Put`s to perform before issuing a `RegisterMsg::Get`.

## 0.25.0

I was recently asked where linearizability was specified in the actor examples.
The answer was `RegisterCfg::into_model`, but this question provided useful
feedback: the existing structure was hiding information important for
understanding each model.

This release addresses that gap by removing `RegisterCfg`. Instead the actor
examples fully specify their `ActorModel`s. This requirement is made less
verbose by introducing two helpers, `RegisterMsg::record_invocations` and
`RegisterMsg::record_returns`.

Here is a representative example prior to this release:

```rust
RegisterCfg {
        client_count: 2,
		servers: vec![
			PaxosActor { peer_ids: model_peers(0, 3) },
			PaxosActor { peer_ids: model_peers(1, 3) },
			PaxosActor { peer_ids: model_peers(2, 3) },
		],
    }
    .into_model()
    .duplicating_network(DuplicatingNetwork::No)
    .within_boundary(within_boundary)
    .checker()
```

As mentioned above, `RegisterCfg::into_model` was implemented inside the
library, thereby inadvertantly hiding important details about the resulting
model. Furthermore, `within_boundary` lacked access to sufficient
configuration:

```rust
fn within_boundary<H>(
    _: &RegisterCfg<PaxosActor>,
    state: &ActorModelState<RegisterActor<PaxosActor>, H>)
    -> bool
{
    state.actor_states.iter().all(|s| {
        if let RegisterActorState::Server(s) = &**s {
            s.ballot.0 <= 3
        } else {
            true
        }
    })
}
```

The new pattern is to introduce a model-specific configuration type, which has
the added benefit of enabling additional model-specific parameters:

```rust
PaxosModelCfg {
        client_count: 2,
        server_count: 3,
        max_round: 3,
    }
    .into_model().checker()
```

The same program would then specify a dedicated `into_model`, thereby
addressing the motivating question of this release. For example:

```rust
impl PaxosModelCfg {
    fn into_model(self) ->
        ActorModel<
            RegisterActor<PaxosActor>,
            Self,
            LinearizabilityTester<Id, Register<Value>>>
    {
        ActorModel::new(
                self.clone(),
                LinearizabilityTester::new(Register(Value::default()))
            )
            .actors((0..self.server_count)
                    .map(|i| RegisterActor::Server(PaxosActor {
                        peer_ids: model_peers(i, self.server_count),
                    })))
            .actors((0..self.client_count)
                    .map(|_| RegisterActor::Client {
                        server_count: self.server_count,
                    }))
            .duplicating_network(DuplicatingNetwork::No)
            .property(Expectation::Always, "linearizable", |_, state| {
                state.history.serialized_history().is_some()
            })
            .property(Expectation::Sometimes, "value chosen", |_, state| {
                for env in &state.network {
                    if let RegisterMsg::GetOk(_req_id, value) = env.msg {
                        if value != Value::default() { return true; }
                    }
                }
                false
            })
            .record_msg_in(RegisterMsg::record_returns)
            .record_msg_out(RegisterMsg::record_invocations)
            .within_boundary(|cfg, state| {
                state.actor_states.iter().all(|s| {
                    if let RegisterActorState::Server(s) = &**s {
                        s.ballot.0 <= cfg.max_round
                    } else {
                        true
                    }
                })
            })
    }
}
```

The release also introduces a supporting `ConsistencyTester` trait generalizing
`LinearizabilityTester` and `SequentialConsistencyTester`. This new trait
enables helpers (such as `RegisterMsg::record_invocations`) to be used with
models expecting linearizability, sequential consistency, or other to-be-added
consistency semantics.
