//! An ordered reliable link (ORL) based loosely on the "perfect link" described
//! in "Introduction to Reliable and Secure Distributed Programming" by Cachin,
//! Guerraoui, and Rodrigues (with enhancements to provide ordering).
//!
//! Order is maintained for messages between a source/destination pair. Order
//! is not maintained between different destinations or different sources.

use crate::actor::*;
use crate::util::HashableHashMap;
use std::fmt::Debug;
use std::time::Duration;
use std::ops::Range;
use std::hash::Hash;

/// Wraps an actor with logic to:
/// 1. Maintain message order.
/// 2. Resend lost messages.
/// 3. Avoid message redelivery.
#[derive(Clone)]
pub struct ActorWrapper<A: Actor> {
    pub resend_interval: Range<Duration>,
    pub wrapped_actor: A,
}

/// An envelope for ORL messages.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum MsgWrapper<Msg> {
    Deliver(Sequencer, Msg),
    Ack(Sequencer),
}

/// Message sequencer.
pub type Sequencer = u64;

/// Maintains state for the ORL.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StateWrapper<Msg, State> {
    // send side
    next_send_seq: Sequencer,
    msgs_pending_ack: HashableHashMap<Sequencer, (Id, Msg)>,

    // receive (ack'ing) side
    last_delivered_seqs: HashableHashMap<Id, Sequencer>,

    wrapped_state: State,
}

impl<A: Actor> ActorWrapper<A> {
    pub fn with_default_timeout(wrapped_actor: A) -> Self {
        Self {
            resend_interval: Duration::from_secs(1)..Duration::from_secs(2),
            wrapped_actor,
        }
    }
}

impl<A: Actor> Actor for ActorWrapper<A>
    where A::Msg: Hash
{
    type Msg = MsgWrapper<A::Msg>;
    type State = StateWrapper<A::Msg, A::State>;

    fn on_start(&self, id: Id, o: &mut Out<Self>) {
        o.set_timer(self.resend_interval.clone());

        let mut wrapped_out = self.wrapped_actor.on_start_out(id);
        let state = StateWrapper {
            next_send_seq: 1,
            msgs_pending_ack: Default::default(),
            last_delivered_seqs: Default::default(),
            wrapped_state: wrapped_out.state.take()
                .unwrap_or_else(|| panic!("on_start must assign state. id={:?}", id)),
        };
        process_output(wrapped_out, state, o);
    }

    fn on_msg(&self, id: Id, state: &Self::State, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
        match msg {
            MsgWrapper::Deliver(seq, wrapped_msg) => {
                // Always ack the message to prevent re-sends, and early exit if already delivered.
                o.send(src, MsgWrapper::Ack(seq));
                if seq <= *state.last_delivered_seqs.get(&src).unwrap_or(&0) { return }

                // Process the message, and early exit if ignored.
                let wrapped_out = self.wrapped_actor.on_msg_out(id, &state.wrapped_state, src, wrapped_msg);
                if wrapped_out.is_no_op() { return }

                // Never delivered, and not ignored by actor, so update the sequencer and process the original output.
                let mut state = state.clone();
                state.last_delivered_seqs.insert(src, seq);
                process_output(wrapped_out, state, o);
            },
            MsgWrapper::Ack(seq) => {
                if !state.msgs_pending_ack.contains_key(&seq) { return }
                let mut state = state.clone();
                state.msgs_pending_ack.remove(&seq);
                o.set_state(state);
            },
        }
    }

    fn on_timeout(&self, _id: Id, state: &Self::State, o: &mut Out<Self>) {
        o.set_timer(self.resend_interval.clone());
        for (seq, (dst, msg)) in &state.msgs_pending_ack {
            o.send(*dst, MsgWrapper::Deliver(*seq, msg.clone()));
        }
    }
}

fn process_output<A: Actor>(wrapped_out: Out<A>, mut state: StateWrapper<A::Msg, A::State>, o: &mut Out<ActorWrapper<A>>)
where A::Msg: Hash
{
    if let Some(wrapped_state) = wrapped_out.state {
        state.wrapped_state = wrapped_state;
    }
    for command in wrapped_out.commands {
        match command {
            Command::CancelTimer => {
                todo!("CancelTimer is not supported at this time");
            },
            Command::SetTimer(_) => {
                todo!("SetTimer is not supported at this time");
            },
            Command::Send(dst, inner_msg) => {
                o.send(dst, MsgWrapper::Deliver(state.next_send_seq, inner_msg.clone()));
                state.msgs_pending_ack.insert(state.next_send_seq, (dst, inner_msg));
                state.next_send_seq += 1;
            },
        }
    }
    o.set_state(state);
}

#[cfg(test)]
mod test {
    use crate::{Property, Model, ModelChecker};
    use crate::actor::{Actor, Id, Out};
    use crate::actor::ordered_reliable_link::{ActorWrapper, MsgWrapper};
    use crate::actor::system::{SystemModel, System, LossyNetwork, DuplicatingNetwork, SystemState};
    use crate::actor::system::SystemAction;

    pub enum TestActor {
        Sender { receiver_id: Id },
        Receiver,
    }
    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    pub struct Received(Vec<(Id, TestMsg)>);
    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    pub struct TestMsg(u64);

    impl Actor for TestActor {
        type Msg = TestMsg;
        type State = Received;

        fn on_start(&self, _id: Id, o: &mut Out<Self>) {
            let state = Received(Vec::new());
            if let TestActor::Sender { receiver_id } = self {
                o.send(*receiver_id, TestMsg(42));
                o.send(*receiver_id, TestMsg(43));
            }
            o.set_state(state);
        }

        fn on_msg(&self, _id: Id, received: &Self::State, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
            let mut received = received.clone();
            received.0.push((src, msg));
            o.set_state(received);
        }
    }

    struct TestSystem;
    impl System for TestSystem {
        type Actor = ActorWrapper<TestActor>;
        type History = ();

        fn actors(&self) -> Vec<Self::Actor> {
            vec![
                ActorWrapper::with_default_timeout(
                    TestActor::Sender { receiver_id: Id::from(1) }),
                ActorWrapper::with_default_timeout(
                    TestActor::Receiver),
            ]
        }

        fn lossy_network(&self) -> LossyNetwork {
            LossyNetwork::Yes
        }

        fn duplicating_network(&self) -> DuplicatingNetwork {
            DuplicatingNetwork::Yes
        }

        fn properties(&self) -> Vec<Property<SystemModel<Self>>> {
            vec![
                Property::<SystemModel<TestSystem>>::always("no redelivery", |_, state| {
                    let received = &state.actor_states[1].wrapped_state.0;
                    received.iter().filter(|(_, TestMsg(v))| *v == 42).count() < 2
                        && received.iter().filter(|(_, TestMsg(v))| *v == 43).count() < 2
                }),
                Property::<SystemModel<TestSystem>>::always("ordered", |_, state| {
                    state.actor_states[1].wrapped_state.0.iter()
                        .map(|(_, TestMsg(v))| *v)
                        .fold((true, 0), |(acc, last), next| (acc && last <= next, next))
                        .0
                }),
                // FIXME: convert to an eventually property once the liveness checker is complete
                Property::<SystemModel<TestSystem>>::sometimes("delivered", |_, state| {
                    state.actor_states[1].wrapped_state.0 == vec![
                        (Id::from(0), TestMsg(42)),
                        (Id::from(0), TestMsg(43)),
                    ]
                }),
            ]
        }

        fn within_boundary(&self, state: &SystemState<Self>) -> bool {
            state.actor_states.iter().all(|s| s.wrapped_state.0.len() < 4)
        }
    }

    #[test]
    fn messages_are_not_delivered_twice() {
        let mut checker = TestSystem.into_model().checker();
        checker.check(10_000).assert_no_counterexample("no redelivery");
    }

    #[test]
    fn messages_are_delivered_in_order() {
        let mut checker = TestSystem.into_model().checker();
        checker.check(10_000).assert_no_counterexample("ordered");
    }

    #[test]
    fn messages_are_eventually_delivered() {
        let mut checker = TestSystem.into_model().checker();
        assert_eq!(
            checker.check(10_000).assert_example("delivered").into_actions(),
            vec![
                SystemAction::Deliver { src: Id(0), dst: Id(1), msg: MsgWrapper::Deliver(1, TestMsg(42)) },
                SystemAction::Deliver { src: Id(0), dst: Id(1), msg: MsgWrapper::Deliver(2, TestMsg(43)) },
            ]);
    }
}
