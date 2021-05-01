# Changes

## 0.26.1

This commit introduces a `PathRecorder` visitor and a `VectorClock` utility
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
