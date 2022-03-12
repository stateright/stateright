//! An ordered reliable link (ORL) based loosely on the "perfect link" described in "[Introduction
//! to Reliable and Secure Distributed
//! Programming](https://link.springer.com/book/10.1007/978-3-642-15260-3)" by Cachin, Guerraoui,
//! and Rodrigues (with enhancements to provide ordering).
//!
//! Order is maintained for messages between a source/destination pair. Order is not maintained
//! between different destinations or different sources.
//!
//! The implementation assumes that actors cannot restart. A later version could persist sequencer
//! information to properly handle actor restarts.
//!
//! # See Also
//!
//! [`Network::new_ordered`] can be used to reduce the state space of models that will use this
//! abstraction.

use crate::actor::*;
use crate::util::HashableHashMap;
use std::borrow::Cow;
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
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

    fn on_start(&self, id: Id, o: &mut Out<Self>) -> Self::State {
        o.set_timer(self.resend_interval.clone());

        let mut wrapped_out = Out::new();
        let mut state = StateWrapper {
            next_send_seq: 1,
            msgs_pending_ack: Default::default(),
            last_delivered_seqs: Default::default(),
            wrapped_state: self.wrapped_actor.on_start(id, &mut wrapped_out),
        };
        process_output(&mut state, wrapped_out, o);
        state
    }

    fn on_msg(&self, id: Id, state: &mut Cow<Self::State>, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
        match msg {
            MsgWrapper::Deliver(seq, wrapped_msg) => {
                // Always ack the message to prevent re-sends, and early exit if already delivered.
                o.send(src, MsgWrapper::Ack(seq));
                if seq <= *state.last_delivered_seqs.get(&src).unwrap_or(&0) { return }

                // Process the message, and early exit if ignored.
                let mut wrapped_state = Cow::Borrowed(&state.wrapped_state);
                let mut wrapped_out = Out::new();
                self.wrapped_actor.on_msg(
                    id, &mut wrapped_state, src, wrapped_msg, &mut wrapped_out);
                if is_no_op(&wrapped_state, &wrapped_out) { return }

                // Never delivered, and not ignored by actor, so update the sequencer and process the original output.
                if let Cow::Owned(wrapped_state) = wrapped_state {
                    // Avoid unnecessarily cloning wrapped_state by not calling to_mut() in this
                    // case.
                    *state = Cow::Owned(StateWrapper {
                        next_send_seq: state.next_send_seq,
                        msgs_pending_ack: state.msgs_pending_ack.clone(),
                        last_delivered_seqs: state.last_delivered_seqs.clone(),
                        wrapped_state,
                    });
                }
                state.to_mut().last_delivered_seqs.insert(src, seq);
                process_output(state.to_mut(), wrapped_out, o);
            },
            MsgWrapper::Ack(seq) => {
                state.to_mut().msgs_pending_ack.remove(&seq);
            },
        }
    }

    fn on_timeout(&self, _id: Id, state: &mut std::borrow::Cow<Self::State>, o: &mut Out<Self>) {
        o.set_timer(self.resend_interval.clone());
        for (seq, (dst, msg)) in &state.msgs_pending_ack {
            o.send(*dst, MsgWrapper::Deliver(*seq, msg.clone()));
        }
    }
}

fn process_output<A: Actor>(state: &mut StateWrapper<A::Msg, A::State>, wrapped_out: Out<A>, o: &mut Out<ActorWrapper<A>>)
where A::Msg: Hash
{
    for command in wrapped_out {
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
}

#[cfg(test)]
mod test {
    use std::borrow::Cow;
    use crate::{Checker, Expectation, Model};
    use crate::actor::{Actor, Id, Out};
    use crate::actor::ordered_reliable_link::{ActorWrapper, MsgWrapper};
    use crate::actor::{ActorModel, ActorModelAction, LossyNetwork, Network};

    pub enum TestActor {
        Sender { receiver_id: Id },
        Receiver,
    }
    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    pub struct Received(Vec<(Id, TestMsg)>);
    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct TestMsg(u64);

    impl Actor for TestActor {
        type Msg = TestMsg;
        type State = Received;

        fn on_start(&self, _id: Id, o: &mut Out<Self>) -> Self::State {
            if let TestActor::Sender { receiver_id } = self {
                o.send(*receiver_id, TestMsg(42));
                o.send(*receiver_id, TestMsg(43));
            }
            Received(Vec::new())
        }

        fn on_msg(&self, _id: Id, received: &mut Cow<Self::State>, src: Id, msg: Self::Msg, _o: &mut Out<Self>) {
            received.to_mut().0.push((src, msg));
        }
    }

    fn model() -> ActorModel<ActorWrapper<TestActor>> {
        ActorModel::new((), ())
            .actor(
                ActorWrapper::with_default_timeout(
                    TestActor::Sender { receiver_id: Id::from(1) }))
            .actor(
                ActorWrapper::with_default_timeout(
                    TestActor::Receiver))
            .init_network(Network::new_unordered_duplicating([]))
            .lossy_network(LossyNetwork::Yes)
            .property(Expectation::Always, "no redelivery", |_, state| {
                let received = &state.actor_states[1].wrapped_state.0;
                received.iter().filter(|(_, TestMsg(v))| *v == 42).count() < 2
                    && received.iter().filter(|(_, TestMsg(v))| *v == 43).count() < 2
            })
            .property(Expectation::Always, "ordered", |_, state| {
                state.actor_states[1].wrapped_state.0.iter()
                    .map(|(_, TestMsg(v))| *v)
                    .fold((true, 0), |(acc, last), next| (acc && last <= next, next))
                    .0
            })
            // FIXME: convert to an eventually property once the liveness checker is complete
            .property(Expectation::Sometimes, "delivered", |_, state| {
                state.actor_states[1].wrapped_state.0 == vec![
                    (Id::from(0), TestMsg(42)),
                    (Id::from(0), TestMsg(43)),
                ]
            })
            .within_boundary(|_, state| {
                state.network.len() < 4
            })
    }

    #[test]
    fn messages_are_not_delivered_twice() {
        model().checker().spawn_bfs().join()
            .assert_no_discovery("no redelivery");
    }

    #[test]
    fn messages_are_delivered_in_order() {
        model().checker().spawn_bfs().join()
            .assert_no_discovery("ordered");
    }

    #[test]
    fn messages_are_eventually_delivered() {
        let checker = model().checker().spawn_bfs().join();
        checker.assert_discovery("delivered", vec![
            ActorModelAction::Deliver {
                src: Id(0),
                dst: Id(1),
                msg: MsgWrapper::Deliver(1, TestMsg(42)),
            },
            ActorModelAction::Deliver {
                src: Id(0),
                dst: Id(1),
                msg: MsgWrapper::Deliver(2, TestMsg(43)),
            },
        ]);
    }
}
