//! Private module for selective re-export.

use crate::{Expectation, Model, Path, Property};
use crate::actor::{
    Actor,
    ActorModelState,
    Command,
    is_no_op,
    Id,
    Out,
};
use crate::util::HashableHashSet;
use std::borrow::Cow;
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

/// Represents a system of [`Actor`]s that communicate over a network. `H` indicates the type of
/// history to maintain as auxiliary state, if any.  See [Auxiliary Variables in
/// TLA](https://lamport.azurewebsites.net/tla/auxiliary/auxiliary.html) for a thorough
/// introduction to that concept. Use `()` if history is not needed to define the relevant
/// properties of this system.
pub struct ActorModel<A, C = (), H = ()>
where A: Actor,
      H: Clone + Debug + Hash,
{
    pub actors: Vec<A>,
    pub cfg: C,
    pub duplicating_network: DuplicatingNetwork,
    pub init_history: H,
    pub init_network: Vec<Envelope<A::Msg>>,
    pub lossy_network: LossyNetwork,
    pub properties: Vec<Property<ActorModel<A, C, H>>>,
    pub record_msg_in: fn(cfg: &C, history: &H, envelope: Envelope<&A::Msg>) -> Option<H>,
    pub record_msg_out: fn(cfg: &C, history: &H, envelope: Envelope<&A::Msg>) -> Option<H>,
    pub within_boundary: fn(cfg: &C, state: &ActorModelState<A, H>) -> bool,
}

/// Indicates possible steps that an actor system can take as it evolves.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ActorModelAction<Msg> {
    /// A message can be delivered to an actor.
    Deliver { src: Id, dst: Id, msg: Msg },
    /// A message can be dropped if the network is lossy.
    Drop(Envelope<Msg>),
    /// An actor can by notified after a timeout.
    Timeout(Id),
}

/// Indicates whether the network duplicates messages. If duplication is disabled, messages
/// are forgotten once delivered, which can improve model checking performance.
#[derive(Copy, Clone, PartialEq)]
pub enum DuplicatingNetwork { Yes, No }

/// Indicates the source and destination for a message.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[derive(serde::Serialize)]
pub struct Envelope<Msg> { pub src: Id, pub dst: Id, pub msg: Msg }

/// Indicates whether the network loses messages. Note that as long as invariants do not check
/// the network state, losing a message is indistinguishable from an unlimited delay, so in
/// many cases you can improve model checking performance by not modeling message loss.
#[derive(Copy, Clone, PartialEq)]
pub enum LossyNetwork { Yes, No }

/// Represents a network of messages.
pub type Network<Msg> = HashableHashSet<Envelope<Msg>>;

/// The specific timeout value is not relevant for model checking, so this helper can be used to
/// generate an arbitrary timeout range. The specific value is subject to change, so this helper
/// must only be used for model checking.
pub fn model_timeout() -> Range<Duration> {
    Duration::from_micros(0)..Duration::from_micros(0)
}

/// A helper to generate a list of peer [`Id`]s given an actor count and the index of a particular
/// actor.
pub fn model_peers(self_ix: usize, count: usize) -> Vec<Id> {
    (0..count)
        .filter(|j| *j != self_ix)
        .map(Into::into)
        .collect()
}

impl<A, C, H> ActorModel<A, C, H>
where A: Actor,
      H: Clone + Debug + Hash,
{
    /// Initializes an [`ActorModel`] with a specified configuration and history.
    pub fn new(cfg: C, init_history: H) -> ActorModel<A, C, H> {
        ActorModel {
            actors: Vec::new(),
            cfg,
            duplicating_network: DuplicatingNetwork::Yes,
            init_history,
            init_network: Default::default(),
            lossy_network: LossyNetwork::No,
            properties: Default::default(),
            record_msg_in: |_, _, _| None,
            record_msg_out: |_, _, _| None,
            within_boundary: |_, _| true,
        }
    }

    /// Adds another [`Actor`] to this model.
    pub fn actor(mut self, actor: A) -> Self {
        self.actors.push(actor);
        self
    }

    /// Adds multiple [`Actor`]s to this model.
    pub fn actors(mut self, actors: impl IntoIterator<Item=A>) -> Self {
        for actor in actors {
            self.actors.push(actor);
        }
        self
    }

    /// Defines whether the network duplicates messages or not.
    pub fn duplicating_network(mut self, duplicating_network: DuplicatingNetwork) -> Self {
        self.duplicating_network = duplicating_network;
        self
    }

    /// Defines the initial network.
    pub fn init_network(mut self, init_network: Vec<Envelope<A::Msg>>) -> Self {
        self.init_network = init_network;
        self
    }

    /// Defines whether the network loses messages or not.
    pub fn lossy_network(mut self, lossy_network: LossyNetwork) -> Self {
        self.lossy_network = lossy_network;
        self
    }

    /// Adds a [`Property`] to this model.
    pub fn property(mut self, expectation: Expectation, name: &'static str,
                     condition: fn(&ActorModel<A, C, H>, &ActorModelState<A, H>) -> bool) -> Self {
        self.properties.push(Property { expectation, name, condition });
        self
    }

    /// Defines whether/how an incoming message contributes to relevant history. Returning
    /// `Some(new_history)` updates the relevant history, while `None` does not.
    pub fn record_msg_in(mut self,
                         record_msg_in: fn(cfg: &C, history: &H, Envelope<&A::Msg>) -> Option<H>)
        -> Self
    {
        self.record_msg_in = record_msg_in;
        self
    }

    /// Defines whether/how an outgoing message contributes to relevant history. Returning
    /// `Some(new_history)` updates the relevant history, while `None` does not.
    pub fn record_msg_out(mut self,
                          record_msg_out: fn(cfg: &C, history: &H, Envelope<&A::Msg>) -> Option<H>)
        -> Self
    {
        self.record_msg_out = record_msg_out;
        self
    }

    /// Indicates whether a state is within the state space that should be model checked.
    pub fn within_boundary(mut self,
                           within_boundary: fn(cfg: &C, state: &ActorModelState<A, H>) -> bool)
        -> Self
    {
        self.within_boundary = within_boundary;
        self
    }

    // Updates the actor state, sends messages, and configures the timer.
    fn process_commands(&self, id: Id, commands: Out<A>, state: &mut ActorModelState<A, H>) {
        let index = usize::from(id);
        for c in commands {
            match c {
                Command::Send(dst, msg) => {
                    if let Some(history) = (self.record_msg_out)(
                        &self.cfg,
                        &state.history,
                        Envelope { src: id, dst, msg: &msg })
                    {
                        state.history = history;
                    }
                    state.network.insert(Envelope { src: id, dst, msg });
                },
                Command::SetTimer(_) => {
                    // must use the index to infer how large as actor state may not be initialized yet
                    if state.is_timer_set.len() <= index {
                        state.is_timer_set.resize(index + 1, false);
                    }
                    state.is_timer_set[index] = true;
                },
                Command::CancelTimer => {
                    state.is_timer_set[index] = false;
                },
            }
        }
    }
}

impl<A, C, H> Model for ActorModel<A, C, H>
where A: Actor,
      H: Clone + Debug + Hash,
{
    type State = ActorModelState<A, H>;
    type Action = ActorModelAction<A::Msg>;

    fn init_states(&self) -> Vec<Self::State> {
        let mut init_sys_state = ActorModelState {
            actor_states: Vec::with_capacity(self.actors.len()),
            history: self.init_history.clone(),
            is_timer_set: Vec::new(),
            network: Network::with_hasher(
                crate::stable::build_hasher()), // for consistent discoveries
        };

        // init the network
        for e in self.init_network.clone() {
            init_sys_state.network.insert(e);
        }

        // init each actor
        for (index, actor) in self.actors.iter().enumerate() {
            let id = Id::from(index);
            let mut out = Out::new();
            let state = actor.on_start(id, &mut out);
            init_sys_state.actor_states.push(Arc::new(state));
            self.process_commands(id, out, &mut init_sys_state);
        }

        vec![init_sys_state]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        for env in &state.network {
            // option 1: message is lost
            if self.lossy_network == LossyNetwork::Yes {
                actions.push(ActorModelAction::Drop(env.clone()));
            }

            // option 2: message is delivered
            if usize::from(env.dst) < self.actors.len() {
                actions.push(ActorModelAction::Deliver { src: env.src, dst: env.dst, msg: env.msg.clone() });
            }
        }

        // option 3: actor timeout
        for (index, &is_scheduled) in state.is_timer_set.iter().enumerate() {
            if is_scheduled {
                actions.push(ActorModelAction::Timeout(Id::from(index)));
            }
        }
    }

    fn next_state(&self, last_sys_state: &Self::State, action: Self::Action) -> Option<Self::State> {
        match action {
            ActorModelAction::Drop(env) => {
                let mut next_state = last_sys_state.clone();
                next_state.network.remove(&env);
                Some(next_state)
            },
            ActorModelAction::Deliver { src, dst: id, msg } => {
                let index = usize::from(id);
                let last_actor_state = &last_sys_state.actor_states.get(index);

                // Not all messags can be delivered, so ignore those.
                if last_actor_state.is_none() { return None; }
                let last_actor_state = &**last_actor_state.unwrap();
                let mut state = Cow::Borrowed(last_actor_state);

                // Some operations are no-ops, so ignore those as well.
                let mut out = Out::new();
                self.actors[index].on_msg(id, &mut state, src, msg.clone(), &mut out);
                if is_no_op(&state, &out) { return None; }
                let history = (self.record_msg_in)(
                    &self.cfg,
                    &last_sys_state.history,
                    Envelope { src, dst: id, msg: &msg });

                // Update the state as necessary:
                // - Drop delivered message if not a duplicating network.
                // - Swap out revised actor state.
                // - Track message input history.
                // - Handle effect of commands on timers, network, and message output history.
                let mut next_sys_state = last_sys_state.clone();
                if self.duplicating_network == DuplicatingNetwork::No {
                    // Strictly speaking, this state should be updated regardless of whether the
                    // actor and history updates are a no-op. The current implementation is only
                    // safe if invariants do not relate to the existence of envelopes on the
                    // network.
                    let env = Envelope { src, dst: id, msg };
                    next_sys_state.network.remove(&env);
                }
                if let Cow::Owned(next_actor_state) = state {
                    next_sys_state.actor_states[index] = Arc::new(next_actor_state);
                }
                if let Some(history) = history {
                    next_sys_state.history = history;
                }
                self.process_commands(id, out, &mut next_sys_state);
                Some(next_sys_state)
            },
            ActorModelAction::Timeout(id) => {
                // Clone new state if necessary (otherwise early exit).
                let index = usize::from(id);
                let mut state = Cow::Borrowed(&*last_sys_state.actor_states[index]);
                let mut out = Out::new();
                self.actors[index].on_timeout(id, &mut state, &mut out);
                let keep_timer = out.iter().any(|c| matches!(c, Command::SetTimer(_)));
                if is_no_op(&state, &out) && keep_timer { return None }
                let mut next_sys_state = last_sys_state.clone();

                // Timer is no longer valid.
                next_sys_state.is_timer_set[index] = false;

                if let Cow::Owned(next_actor_state) = state {
                    next_sys_state.actor_states[index] = Arc::new(next_actor_state);
                }
                self.process_commands(id, out, &mut next_sys_state);
                Some(next_sys_state)
            },
        }
    }

    fn display_outcome(&self, last_state: &Self::State, action: Self::Action) -> Option<String>
    where Self::State: Debug
    {
        struct ActorStep<'a, A: Actor> {
            last_state: &'a A::State,
            next_state: Option<A::State>,
            out: Out<A>,
        }
        impl<'a, A: Actor> Display for ActorStep<'a, A> {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                writeln!(f, "OUT: {:?}", self.out)?;
                writeln!(f)?;
                if let Some(next_state) = &self.next_state {
                    writeln!(f, "NEXT_STATE: {:#?}", next_state)?;
                    writeln!(f)?;
                    writeln!(f, "PREV_STATE: {:#?}", self.last_state)
                } else {
                    writeln!(f, "UNCHANGED: {:#?}", self.last_state)
                }
            }
        }

        match action {
            ActorModelAction::Drop(env) => {
                Some(format!("DROP: {:?}", env))
            },
            ActorModelAction::Deliver { src, dst: id, msg } => {
                let index = usize::from(id);
                let last_actor_state = match last_state.actor_states.get(index) {
                    None => return None,
                    Some(last_actor_state) => &**last_actor_state,
                };
                let mut actor_state = Cow::Borrowed(last_actor_state);
                let mut out = Out::new();
                self.actors[index].on_msg(id, &mut actor_state, src, msg, &mut out);
                Some(format!("{}", ActorStep {
                    last_state: last_actor_state,
                    next_state: match actor_state {
                        Cow::Borrowed(_) => None,
                        Cow::Owned(next_actor_state) => Some(next_actor_state),
                    },
                    out,
                }))
            },
            ActorModelAction::Timeout(id) => {
                let index = usize::from(id);
                let last_actor_state = match last_state.actor_states.get(index) {
                    None => return None,
                    Some(last_actor_state) => &**last_actor_state,
                };
                let mut actor_state = Cow::Borrowed(last_actor_state);
                let mut out = Out::new();
                self.actors[index].on_timeout(id, &mut actor_state, &mut out);
                Some(format!("{}", ActorStep {
                    last_state: last_actor_state,
                    next_state: match actor_state {
                        Cow::Borrowed(_) => None,
                        Cow::Owned(next_actor_state) => Some(next_actor_state),
                    },
                    out,
                }))
            },
        }
    }

    /// Draws a sequence diagram for the actor system.
    fn as_svg(&self, path: Path<Self::State, Self::Action>) -> Option<String> {
        use std::collections::HashMap;
        use std::fmt::Write;

        let plot = |x, y| (x as u64 * 100, y as u64 * 30);
        let actor_count = path.last_state().actor_states.len();
        let path = path.into_vec();

        // SVG wrapper.
        let (mut svg_w, svg_h) = plot(actor_count, path.len());
        svg_w += 300; // KLUDGE: extra width for event labels
        let mut svg = format!("<svg version='1.1' baseProfile='full' \
                                    width='{}' height='{}' viewbox='-20 -20 {} {}' \
                                    xmlns='http://www.w3.org/2000/svg'>",
            svg_w, svg_h, svg_w + 20, svg_h + 20);

        // Definitions.
        write!(&mut svg, "\
            <defs>\
              <marker class='svg-event-shape' id='arrow' markerWidth='12' markerHeight='10' refX='12' refY='5' orient='auto'>\
                <polygon points='0 0, 12 5, 0 10' />\
              </marker>\
            </defs>").unwrap();

        // Vertical timeline for each actor.
        for actor_index in 0..actor_count {
            let (x1, y1) = plot(actor_index, 0);
            let (x2, y2) = plot(actor_index, path.len());
            writeln!(&mut svg, "<line x1='{}' y1='{}' x2='{}' y2='{}' class='svg-actor-timeline' />",
                   x1, y1, x2, y2).unwrap();
            writeln!(&mut svg, "<text x='{}' y='{}' class='svg-actor-label'>{:?}</text>",
                   x1, y1, actor_index).unwrap();
        }

        // Arrow for each delivery. Circle for other events.
        let mut send_time  = HashMap::new();
        for (time, (state, action)) in path.clone().into_iter().enumerate() {
            let time = time + 1; // action is for the next step
            match action {
                Some(ActorModelAction::Deliver { src, dst: id, msg }) => {
                    let src_time = *send_time.get(&(src, id, msg.clone())).unwrap_or(&0);
                    let (x1, y1) = plot(src.into(), src_time);
                    let (x2, y2) = plot(id.into(),  time);
                    writeln!(&mut svg, "<line x1='{}' x2='{}' y1='{}' y2='{}' marker-end='url(#arrow)' class='svg-event-line' />",
                           x1, x2, y1, y2).unwrap();

                    // Track sends to facilitate building arrows.
                    let index = usize::from(id);
                    if let Some(actor_state) = state.actor_states.get(index) {
                        let mut actor_state = Cow::Borrowed(&**actor_state);
                        let mut out = Out::new();
                        self.actors[index].on_msg(id, &mut actor_state, src, msg, &mut out);
                        for command in out {
                            if let Command::Send(dst, msg) = command {
                                send_time.insert((id, dst, msg), time);
                            }
                        }
                    }
                }
                Some(ActorModelAction::Timeout(actor_id)) => {
                    let (x, y) = plot(actor_id.into(),  time);
                    writeln!(&mut svg, "<circle cx='{}' cy='{}' r='10' class='svg-event-shape' />",
                           x, y).unwrap();

                    // Track sends to facilitate building arrows.
                    let index = usize::from(actor_id);
                    if let Some(actor_state) = state.actor_states.get(index) {
                        let mut actor_state = Cow::Borrowed(&**actor_state);
                        let mut out = Out::new();
                        self.actors[index].on_timeout(actor_id, &mut actor_state, &mut out);
                        for command in out {
                            if let Command::Send(dst, msg) = command {
                                send_time.insert((actor_id, dst, msg), time);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Handle event labels last to ensure they are drawn over shapes.
        for (time, (_state, action)) in path.into_iter().enumerate() {
            let time = time + 1; // action is for the next step
            match action {
                Some(ActorModelAction::Deliver { dst: id, msg, .. }) => {
                    let (x, y) = plot(id.into(), time);
                    writeln!(&mut svg, "<text x='{}' y='{}' class='svg-event-label'>{:?}</text>",
                           x, y, msg).unwrap();
                }
                Some(ActorModelAction::Timeout(id)) => {
                    let (x, y) = plot(id.into(), time);
                    writeln!(&mut svg, "<text x='{}' y='{}' class='svg-event-label'>Timeout</text>",
                           x, y).unwrap();
                }
                _ => {}
            }
        }

        writeln!(&mut svg, "</svg>").unwrap();
        Some(svg)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        self.properties.clone()
    }

    fn within_boundary(&self, state: &Self::State) -> bool {
        (self.within_boundary)(&self.cfg, state)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{Checker, StateRecorder};
    use crate::actor::actor_test_util::ping_pong::{PingPongCfg, PingPongMsg::*};
    use crate::actor::ActorModelAction::*;
    use std::collections::HashSet;
    use std::sync::Arc;


    #[test]
    fn visits_expected_states() {
        use std::iter::FromIterator;

        // helper to make the test more concise
        let states_and_network = |states: Vec<u32>, envelopes: Vec<Envelope<_>>| {
            ActorModelState {
                actor_states: states.into_iter().map(|s| Arc::new(s)).collect::<Vec<_>>(),
                network: Network::from_iter(envelopes),
                is_timer_set: Vec::new(),
                history: (0_u32, 0_u32), // constant as `maintains_history: false`
            }
        };

        let (recorder, accessor) = StateRecorder::new_with_accessor();
        let checker = PingPongCfg {
                maintains_history: false,
                max_nat: 1,
            }
            .into_model()
            .lossy_network(LossyNetwork::Yes)
            .checker().visitor(recorder).spawn_bfs().join();
        assert_eq!(checker.generated_count(), 14);

        let state_space = accessor();
        assert_eq!(state_space.len(), 14); // same as the generated count
        assert_eq!(HashSet::<_>::from_iter(state_space), HashSet::from_iter(vec![
            // When the network loses no messages...
            states_and_network(
                vec![0, 0],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) }]),
            states_and_network(
                vec![0, 1],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                ]),
            states_and_network(
                vec![1, 1],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) },
                ]),

            // When the network loses the message for pinger-ponger state (0, 0)...
            states_and_network(
                vec![0, 0],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (0, 1)
            states_and_network(
                vec![0, 1],
                vec![Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) }]),
            states_and_network(
                vec![0, 1],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) }]),
            states_and_network(
                vec![0, 1],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (1, 1)
            states_and_network(
                vec![1, 1],
                vec![
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) },
                ]),
            states_and_network(
                vec![1, 1],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) },
                ]),
            states_and_network(
                vec![1, 1],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                ]),
            states_and_network(
                vec![1, 1],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) }]),
            states_and_network(
                vec![1, 1],
                vec![Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) }]),
            states_and_network(
                vec![1, 1],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) }]),
            states_and_network(
                vec![1, 1],
                Vec::new()),
        ]));
    }

    #[test]
    fn maintains_fixed_delta_despite_lossy_duplicating_network() {
        let checker = PingPongCfg {
                max_nat: 5,
                maintains_history: false,
            }
            .into_model()
            .lossy_network(LossyNetwork::Yes)
            .checker().spawn_bfs().join();
        assert_eq!(checker.generated_count(), 4_094);
        checker.assert_no_discovery("delta within 1");
    }

    #[test]
    fn may_never_reach_max_on_lossy_network() {
        let checker = PingPongCfg {
                max_nat: 5,
                maintains_history: false,
            }
            .into_model()
            .lossy_network(LossyNetwork::Yes)
            .checker().spawn_bfs().join();
        assert_eq!(checker.generated_count(), 4_094);

        // can lose the first message and get stuck, for example
        checker.assert_discovery("must reach max", vec![
            Drop(Envelope { src: Id(0), dst: Id(1), msg: Ping(0) }),
        ]);
    }

    #[test]
    fn eventually_reaches_max_on_perfect_delivery_network() {
        let checker = PingPongCfg {
                max_nat: 5,
                maintains_history: false,
            }
            .into_model()
            .duplicating_network(DuplicatingNetwork::No)
            .lossy_network(LossyNetwork::No)
            .checker().spawn_bfs().join();
        assert_eq!(checker.generated_count(), 11);
        checker.assert_no_discovery("must reach max");
    }

    #[test]
    fn can_reach_max() {
        let checker = PingPongCfg {
                max_nat: 5,
                maintains_history: false,
            }
            .into_model()
            .lossy_network(LossyNetwork::No)
            .checker().spawn_bfs().join();
        assert_eq!(checker.generated_count(), 11);
        assert_eq!(
            checker.discovery("can reach max").unwrap().last_state().actor_states,
            vec![Arc::new(4), Arc::new(5)]);
    }

    #[test]
    fn might_never_reach_beyond_max() {
        // ^ and in fact will never. This is a subtle distinction: we're exercising a
        //   falsifiable liveness property here (eventually must exceed max), whereas "will never"
        //   refers to a verifiable safety property (always will not exceed).

        let checker = PingPongCfg {
                max_nat: 5,
                maintains_history: false,
            }
            .into_model()
            .duplicating_network(DuplicatingNetwork::No)
            .lossy_network(LossyNetwork::No)
            .checker().spawn_bfs().join();
        assert_eq!(checker.generated_count(), 11);

        // this is an example of a liveness property that fails to hold (due to the boundary)
        assert_eq!(
            checker.discovery("must exceed max").unwrap().last_state().actor_states,
            vec![Arc::new(5), Arc::new(5)]);
    }

    #[test]
    fn handles_undeliverable_messages() {
        assert_eq!(
            ActorModel::new((), ())
                .actor(())
                .property(Expectation::Always, "unused", |_, _| true) // force full traversal
                .init_network(vec![Envelope { src: 0.into(), dst: 99.into(), msg: () }])
                .checker().spawn_bfs().join()
                .generated_count(),
            1);
    }

    #[test]
    fn resets_timer() {
        struct TestActor;
        impl Actor for TestActor {
            type State = ();
            type Msg = ();
            fn on_start(&self, _: Id, o: &mut Out<Self>) {
                o.set_timer(model_timeout());
                ()
            }
            fn on_msg(&self, _: Id, _: &mut Cow<Self::State>, _: Id, _: Self::Msg, _: &mut Out<Self>) {}
        }

        // Init state with timer, followed by next state without timer.
        assert_eq!(
            ActorModel::new((), ())
                .actor(TestActor)
                .property(Expectation::Always, "unused", |_, _| true) // force full traversal
                .checker().spawn_bfs().join()
                .generated_count(),
            2);
    }
}

#[cfg(test)]
mod choice_test {
    use choice::Choice;
    use crate::{Checker, Model, StateRecorder};
    use crate::actor::DuplicatingNetwork;
    use super::*;

    #[derive(Clone)]
    struct A { b: Id }
    impl Actor for A {
        type State = u8;
        type Msg = ();
        fn on_start(&self, _: Id, _: &mut Out<Self>) -> Self::State {
            1
        }
        fn on_msg(&self, _: Id, state: &mut Cow<Self::State>,
                  _: Id, _: Self::Msg, o: &mut Out<Self>) {
            *state.to_mut() = state.wrapping_add(1);
            o.send(self.b, ());
        }
    }

    #[derive(Clone)]
    struct B { c: Id }
    impl Actor for B {
        type State = char;
        type Msg = ();
        fn on_start(&self, _: Id, _: &mut Out<Self>) -> Self::State {
            'a'
        }
        fn on_msg(&self, _: Id, state: &mut Cow<Self::State>,
                  _: Id, _: Self::Msg, o: &mut Out<Self>) {
            *state.to_mut() = (**state as u8).wrapping_add(1) as char;
            o.send(self.c, ());
        }
    }

    #[derive(Clone)]
    struct C { a: Id }
    impl Actor for C {
        type State = String;
        type Msg = ();
        fn on_start(&self, _: Id, o: &mut Out<Self>) -> Self::State {
            o.send(self.a, ());
            "I".to_string()
        }
        fn on_msg(&self, _: Id, state: &mut Cow<Self::State>,
                  _: Id, _: Self::Msg, o: &mut Out<Self>) {
            state.to_mut().push('I');
            o.send(self.a, ());
        }
    }

    #[test]
    fn choice_correctly_implements_actor() {
        use choice::choice;
        let sys = ActorModel::<choice![A, B, C], (), u8>::new((), 0)
            .actor(Choice::new(A { b: Id::from(1) }))
            .actor(Choice::new(B { c: Id::from(2) }).or())
            .actor(Choice::new(C { a: Id::from(0) }).or().or())
            .duplicating_network(DuplicatingNetwork::No)
            .record_msg_out(|_, out_count, _| Some(out_count + 1))
            .property(Expectation::Always, "true", |_, _| true)
            .within_boundary(|_, state| state.history < 8);
        let (recorder, accessor) = StateRecorder::new_with_accessor();
        sys.checker()
            .visitor(recorder)
            .spawn_dfs().join();
        let states: Vec<Vec<choice![u8, char, String]>> = accessor().into_iter()
            .map(|s| s.actor_states.into_iter().map(|a| (&*a).clone()).collect())
            .collect();
        assert_eq!(states, vec![
            // Init.
            vec![
                Choice::new(1),
                Choice::new('a').or(),
                Choice::new("I".to_string()).or().or(),
            ],
            // Then deliver to A.
            vec![
                Choice::new(2),
                Choice::new('a').or(),
                Choice::new("I".to_string()).or().or(),
            ],
            // Then deliver to B.
            vec![
                Choice::new(2),
                Choice::new('b').or(),
                Choice::new("I".to_string()).or().or(),
            ],
            // Then deliver to C.
            vec![
                Choice::new(2),
                Choice::new('b').or(),
                Choice::new("II".to_string()).or().or(),
            ],
            // Then deliver to A again.
            vec![
                Choice::new(3),
                Choice::new('b').or(),
                Choice::new("II".to_string()).or().or(),
            ],
            // Then deliver to B again.
            vec![
                Choice::new(3),
                Choice::new('c').or(),
                Choice::new("II".to_string()).or().or(),
            ],
            // Then deliver to C again.
            vec![
                Choice::new(3),
                Choice::new('c').or(),
                Choice::new("III".to_string()).or().or(),
            ],
        ]);
    }
}

