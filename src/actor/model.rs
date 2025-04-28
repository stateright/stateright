//! Private module for selective re-export.

use crate::actor::{
    is_no_op, is_no_op_with_timer, Actor, ActorModelState, Command, Envelope, Id, Network, Out,
    RandomChoices,
};
use crate::{Expectation, Model, Path, Property};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use super::timers::Timers;

/// Represents a system of [`Actor`]s that communicate over a network. `H` indicates the type of
/// history to maintain as auxiliary state, if any.  See [Auxiliary Variables in
/// TLA](https://lamport.azurewebsites.net/tla/auxiliary/auxiliary.html) for a thorough
/// introduction to that concept. Use `()` if history is not needed to define the relevant
/// properties of this system.
#[derive(Clone)]
pub struct ActorModel<A, C = (), H = ()>
where
    A: Actor,
    H: Clone + Debug + Hash,
{
    pub actors: Vec<A>,
    pub cfg: C,
    pub init_history: H,
    pub init_network: Network<A::Msg>,
    pub lossy_network: LossyNetwork,
    /// Maximum number of actors that can be contemporarily crashed
    pub max_crashes: usize,
    pub properties: Vec<Property<ActorModel<A, C, H>>>,
    pub record_msg_in: fn(cfg: &C, history: &H, envelope: Envelope<&A::Msg>) -> Option<H>,
    pub record_msg_out: fn(cfg: &C, history: &H, envelope: Envelope<&A::Msg>) -> Option<H>,
    pub within_boundary: fn(cfg: &C, state: &ActorModelState<A, H>) -> bool,
}

/// Indicates possible steps that an actor system can take as it evolves.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ActorModelAction<Msg, Timer, Random> {
    /// A message can be delivered to an actor.
    Deliver {
        src: Id,
        dst: Id,
        msg: Msg,
    },
    /// A message can be dropped if the network is lossy.
    Drop(Envelope<Msg>),
    /// An actor can be notified after a timeout.
    Timeout(Id, Timer),
    /// An actor can crash, i.e. losing all its volatile state.
    Crash(Id),
    /// An actor can recover, i.e. restoring all its non-volatile state in Self::Storage.
    Recover(Id),
    /// A random selection by an actor.
    SelectRandom {
        actor: Id,
        key: String,
        random: Random,
    },
}

/// Indicates whether the network loses messages. Note that as long as invariants do not check
/// the network state, losing a message is indistinguishable from an unlimited delay, so in
/// many cases you can improve model checking performance by not modeling message loss.
#[derive(Copy, Clone, PartialEq)]
pub enum LossyNetwork {
    Yes,
    No,
}

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
where
    A: Actor,
    H: Clone + Debug + Hash,
{
    /// Initializes an [`ActorModel`] with a specified configuration and history.
    pub fn new(cfg: C, init_history: H) -> ActorModel<A, C, H> {
        ActorModel {
            actors: Vec::new(),
            cfg,
            init_history,
            init_network: Network::new_unordered_duplicating([]),
            lossy_network: LossyNetwork::No,
            max_crashes: 0,
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
    pub fn actors(mut self, actors: impl IntoIterator<Item = A>) -> Self {
        for actor in actors {
            self.actors.push(actor);
        }
        self
    }

    /// Defines the initial network.
    pub fn init_network(mut self, init_network: Network<A::Msg>) -> Self {
        self.init_network = init_network;
        self
    }

    /// Defines whether the network loses messages or not.
    pub fn lossy_network(mut self, lossy_network: LossyNetwork) -> Self {
        self.lossy_network = lossy_network;
        self
    }

    /// Specifies the maximum number of actors that can be contemporarily crashed
    pub fn max_crashes(mut self, max_crashes: usize) -> Self {
        self.max_crashes = max_crashes;
        self
    }

    /// Adds a [`Property`] to this model.
    #[allow(clippy::type_complexity)]
    pub fn property(
        mut self,
        expectation: Expectation,
        name: &'static str,
        condition: fn(&ActorModel<A, C, H>, &ActorModelState<A, H>) -> bool,
    ) -> Self {
        self.properties.push(Property {
            expectation,
            name,
            condition,
        });
        self
    }

    /// Defines whether/how an incoming message contributes to relevant history. Returning
    /// `Some(new_history)` updates the relevant history, while `None` does not.
    pub fn record_msg_in(
        mut self,
        record_msg_in: fn(cfg: &C, history: &H, Envelope<&A::Msg>) -> Option<H>,
    ) -> Self {
        self.record_msg_in = record_msg_in;
        self
    }

    /// Defines whether/how an outgoing message contributes to relevant history. Returning
    /// `Some(new_history)` updates the relevant history, while `None` does not.
    pub fn record_msg_out(
        mut self,
        record_msg_out: fn(cfg: &C, history: &H, Envelope<&A::Msg>) -> Option<H>,
    ) -> Self {
        self.record_msg_out = record_msg_out;
        self
    }

    /// Indicates whether a state is within the state space that should be model checked.
    pub fn within_boundary(
        mut self,
        within_boundary: fn(cfg: &C, state: &ActorModelState<A, H>) -> bool,
    ) -> Self {
        self.within_boundary = within_boundary;
        self
    }

    /// Updates the actor state, sends messages, and configures the timers.
    fn process_commands(&self, id: Id, commands: Out<A>, state: &mut ActorModelState<A, H>) {
        let index = usize::from(id);
        for c in commands {
            match c {
                Command::Send(dst, msg) => {
                    if let Some(history) = (self.record_msg_out)(
                        &self.cfg,
                        &state.history,
                        Envelope {
                            src: id,
                            dst,
                            msg: &msg,
                        },
                    ) {
                        state.history = history;
                    }
                    state.network.send(Envelope { src: id, dst, msg });
                }
                Command::SetTimer(timer, _) => {
                    // must use the index to infer how large as actor state may not be initialized yet
                    if state.timers_set.len() <= index {
                        state.timers_set.resize_with(index + 1, Timers::new);
                    }
                    state.timers_set[index].set(timer);
                }
                Command::CancelTimer(timer) => {
                    state.timers_set[index].cancel(&timer);
                }
                Command::ChooseRandom(key, random) => {
                    if random.is_empty() {
                        state.random_choices[index].remove(&key);
                    } else {
                        state.random_choices[index].insert(key, random)
                    }
                }
                Command::Save(storage) => {
                    // must use the index to infer how large as actor state may not be initialized yet
                    if state.actor_storages.len() <= index {
                        state.actor_storages.resize_with(index + 1, Default::default);
                    }
                    state.actor_storages[index] = Some(storage);
                }
            }
        }
    }
}

impl<A, C, H> Model for ActorModel<A, C, H>
where
    A: Actor,
    H: Clone + Debug + Hash,
{
    type State = ActorModelState<A, H>;
    type Action = ActorModelAction<A::Msg, A::Timer, A::Random>;

    fn init_states(&self) -> Vec<Self::State> {
        let mut init_sys_state = ActorModelState {
            actor_states: Vec::with_capacity(self.actors.len()),
            history: self.init_history.clone(),
            timers_set: vec![Timers::new(); self.actors.len()],
            random_choices: vec![RandomChoices::default(); self.actors.len()],
            network: self.init_network.clone(),
            crashed: vec![false; self.actors.len()],
            actor_storages: vec![None; self.actors.len()],
        };

        // init each actor
        for (index, actor) in self.actors.iter().enumerate() {
            let id = Id::from(index);
            let mut out = Out::new();
            let state = actor.on_start(id, &init_sys_state.actor_storages[index], &mut out);
            init_sys_state.actor_states.push(Arc::new(state));
            self.process_commands(id, out, &mut init_sys_state);
        }

        vec![init_sys_state]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        let mut prev_channel = None; // Only deliver the head of a channel.
        for env in state.network.iter_deliverable() {
            // option 1: message is lost
            if self.lossy_network == LossyNetwork::Yes {
                actions.push(ActorModelAction::Drop(env.to_cloned_msg()));
            }

            // option 2: message is delivered
            if usize::from(env.dst) < self.actors.len() {
                // ignored if recipient DNE
                if matches!(self.init_network, Network::Ordered(_)) {
                    let curr_channel = (env.src, env.dst);
                    if prev_channel == Some(curr_channel) {
                        continue;
                    } // queued behind previous
                    prev_channel = Some(curr_channel);
                }
                actions.push(ActorModelAction::Deliver {
                    src: env.src,
                    dst: env.dst,
                    msg: env.msg.clone(),
                });
            }
        }

        // option 3: actor timeout
        for (index, timers) in state.timers_set.iter().enumerate() {
            for timer in timers.iter() {
                actions.push(ActorModelAction::Timeout(Id::from(index), timer.clone()));
            }
        }

        // option 4: actor crash
        let n_crashed = state.crashed.iter().filter(|&crashed| *crashed).count();
        if n_crashed < self.max_crashes {
            state
                .crashed
                .iter()
                .enumerate()
                .filter_map(|(index, &crashed)| if !crashed { Some(index) } else { None })
                .for_each(|index| actions.push(ActorModelAction::Crash(Id::from(index))));
        }

        // option 5: actor recover
        state
            .crashed
            .iter()
            .enumerate()
            .filter_map(|(index, &crashed)| if crashed { Some(index) } else { None })
            .for_each(|index| actions.push(ActorModelAction::Recover(Id::from(index))));

        // option 6: random choice
        for (actor_index, random_decisions) in state.random_choices.iter().enumerate() {
            for (key, decision) in random_decisions.map.into_iter() {
                for choice in decision {
                    actions.push(ActorModelAction::SelectRandom {
                        actor: Id::from(actor_index),
                        key: key.clone(),
                        random: choice.clone(),
                    });
                }
            }
        }
    }

    fn next_state(
        &self,
        last_sys_state: &Self::State,
        action: Self::Action,
    ) -> Option<Self::State> {
        match action {
            ActorModelAction::Drop(env) => {
                let mut next_state = last_sys_state.clone();
                next_state.network.on_drop(env);
                Some(next_state)
            }
            ActorModelAction::Deliver { src, dst: id, msg } => {
                let index = usize::from(id);
                let last_actor_state = &last_sys_state.actor_states.get(index);

                // Not all messages can be delivered, so ignore those.
                if last_actor_state.is_none() {
                    return None;
                }
                if last_sys_state.crashed[index] {
                    return None;
                }

                let last_actor_state = &**last_actor_state.unwrap();
                let mut state = Cow::Borrowed(last_actor_state);

                // Some operations are no-ops, so ignore those as well.
                let mut out = Out::new();
                self.actors[index].on_msg(id, &mut state, src, msg.clone(), &mut out);
                if is_no_op(&state, &out) && !matches!(self.init_network, Network::Ordered(_)) {
                    return None;
                }
                let history = (self.record_msg_in)(
                    &self.cfg,
                    &last_sys_state.history,
                    Envelope {
                        src,
                        dst: id,
                        msg: &msg,
                    },
                );

                // Update the state as necessary:
                // - Drop delivered message if not a duplicating network.
                // - Swap out revised actor state.
                // - Track message input history.
                // - Handle effect of commands on timers, network, and message output history.
                //
                // Strictly speaking, this state should be updated regardless of whether the
                // actor and history updates are a no-op. The current implementation is only
                // safe if invariants do not relate to the existence of envelopes on the
                // network.
                let mut next_sys_state = last_sys_state.clone();
                let env = Envelope { src, dst: id, msg };
                next_sys_state.network.on_deliver(env);
                if let Cow::Owned(next_actor_state) = state {
                    next_sys_state.actor_states[index] = Arc::new(next_actor_state);
                }
                if let Some(history) = history {
                    next_sys_state.history = history;
                }
                self.process_commands(id, out, &mut next_sys_state);
                Some(next_sys_state)
            }
            ActorModelAction::Timeout(id, timer) => {
                // Clone new state if necessary (otherwise early exit).
                let index = usize::from(id);
                let mut state = Cow::Borrowed(&*last_sys_state.actor_states[index]);
                let mut out = Out::new();
                self.actors[index].on_timeout(id, &mut state, &timer, &mut out);
                if is_no_op_with_timer(&state, &out, &timer) {
                    return None;
                }
                let mut next_sys_state = last_sys_state.clone();

                // Timer is no longer valid.
                next_sys_state.timers_set[index].cancel(&timer);

                if let Cow::Owned(next_actor_state) = state {
                    next_sys_state.actor_states[index] = Arc::new(next_actor_state);
                }
                self.process_commands(id, out, &mut next_sys_state);
                Some(next_sys_state)
            }
            ActorModelAction::Crash(id) => {
                let index = usize::from(id);

                let mut next_sys_state = last_sys_state.clone();
                next_sys_state.timers_set[index].cancel_all();
                next_sys_state.random_choices[index].map.clear();
                next_sys_state.crashed[index] = true;

                Some(next_sys_state)
            }
            ActorModelAction::Recover(id) => {
                let index = usize::from(id);
                assert_eq!(last_sys_state.crashed[index], true);
                let mut out = Out::new();
                let state =
                    self.actors[index].on_start(id, &last_sys_state.actor_storages[index], &mut out);
                let mut next_sys_state = last_sys_state.clone();
                next_sys_state.actor_states[index] = Arc::new(state);
                next_sys_state.crashed[index] = false;
                self.process_commands(id, out, &mut next_sys_state);
                Some(next_sys_state)
            }
            ActorModelAction::SelectRandom { actor, key, random } => {
                let actor_index = usize::from(actor);
                let mut state = Cow::Borrowed(&*last_sys_state.actor_states[actor_index]);
                let mut out = Out::new();
                self.actors[actor_index].on_random(actor, &mut state, &random, &mut out);
                let mut next_sys_state = last_sys_state.clone();
                // This random choice is no longer valid.
                next_sys_state.random_choices[actor_index].remove(&key);

                if let Cow::Owned(next_actor_state) = state {
                    next_sys_state.actor_states[actor_index] = Arc::new(next_actor_state);
                }
                self.process_commands(actor, out, &mut next_sys_state);
                Some(next_sys_state)
            }
        }
    }

    fn format_action(&self, action: &Self::Action) -> String {
        match action {
            ActorModelAction::Deliver { src, dst, msg } => {
                format!("{:?} → {:?} → {:?}", src, msg, dst)
            }
            ActorModelAction::SelectRandom {
                actor,
                key: _,
                random,
            } => format!("{actor:?} select random {random:?}"),
            _ => format!("{:?}", action),
        }
    }

    fn format_step(&self, last_state: &Self::State, action: Self::Action) -> Option<String>
    where
        Self::State: Debug,
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
            ActorModelAction::Drop(env) => Some(format!("DROP: {:?}", env)),
            ActorModelAction::Deliver { src, dst: id, msg } => {
                let index = usize::from(id);
                let last_actor_state = match last_state.actor_states.get(index) {
                    None => return None,
                    Some(last_actor_state) => &**last_actor_state,
                };
                let mut actor_state = Cow::Borrowed(last_actor_state);
                let mut out = Out::new();
                self.actors[index].on_msg(id, &mut actor_state, src, msg, &mut out);
                Some(format!(
                    "{}",
                    ActorStep {
                        last_state: last_actor_state,
                        next_state: match actor_state {
                            Cow::Borrowed(_) => None,
                            Cow::Owned(next_actor_state) => Some(next_actor_state),
                        },
                        out,
                    }
                ))
            }
            ActorModelAction::Timeout(id, timer) => {
                let index = usize::from(id);
                let last_actor_state = match last_state.actor_states.get(index) {
                    None => return None,
                    Some(last_actor_state) => &**last_actor_state,
                };
                let mut actor_state = Cow::Borrowed(last_actor_state);
                let mut out = Out::new();
                self.actors[index].on_timeout(id, &mut actor_state, &timer, &mut out);
                Some(format!(
                    "{}",
                    ActorStep {
                        last_state: last_actor_state,
                        next_state: match actor_state {
                            Cow::Borrowed(_) => None,
                            Cow::Owned(next_actor_state) => Some(next_actor_state),
                        },
                        out,
                    }
                ))
            }
            ActorModelAction::Crash(id) => {
                let index = usize::from(id);
                last_state.actor_states.get(index).map(|last_actor_state| {
                    format!(
                        "{}",
                        ActorStep {
                            last_state: &**Cow::Borrowed(last_actor_state),
                            next_state: None,
                            out: Out::new() as Out<A>,
                        }
                    )
                })
            }
            ActorModelAction::Recover(id) => {
                let index = usize::from(id);
                let last_actor_state = match last_state.actor_states.get(index) {
                    None => return None,
                    Some(last_actor_state) => &**last_actor_state,
                };
                let mut out = Out::new();
                let actor_state =
                    self.actors[index].on_start(id, &last_state.actor_storages[index], &mut out);
                Some(format!(
                    "{}",
                    ActorStep {
                        last_state: last_actor_state,
                        next_state: Some(actor_state),
                        out,
                    }
                ))
            }
            ActorModelAction::SelectRandom {
                actor,
                key: _,
                random,
            } => {
                let index = usize::from(actor);
                let last_actor_state = match last_state.actor_states.get(index) {
                    None => return None,
                    Some(last_actor_state) => &**last_actor_state,
                };
                let mut actor_state = Cow::Borrowed(last_actor_state);
                let mut out = Out::new();
                self.actors[index].on_random(actor, &mut actor_state, &random, &mut out);
                Some(format!(
                    "{}",
                    ActorStep {
                        last_state: last_actor_state,
                        next_state: match actor_state {
                            Cow::Borrowed(_) => None,
                            Cow::Owned(next_actor_state) => Some(next_actor_state),
                        },
                        out,
                    }
                ))
            }
        }
    }

    /// Draws a sequence diagram for the actor system.
    fn as_svg(&self, path: Path<Self::State, Self::Action>) -> Option<String> {
        use std::fmt::Write;

        let approximate_letter_width_px = 10;
        let actor_names = self
            .actors
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let name = a.name();
                if name.is_empty() {
                    i.to_string()
                } else {
                    format!("{} {}", i, name)
                }
            })
            .collect::<Vec<_>>();
        let max_name_len = actor_names
            .iter()
            .map(|n| n.len() as u64)
            .max()
            .unwrap_or_default()
            * approximate_letter_width_px;
        let spacing = std::cmp::max(100, max_name_len);
        let plot = |x, y| (x as u64 * spacing, y as u64 * 30);
        let actor_count = path.last_state().actor_states.len();
        let path = path.into_vec();

        // SVG wrapper.
        let (mut svg_w, svg_h) = plot(actor_count, path.len());
        svg_w += 300; // KLUDGE: extra width for event labels
        let mut svg = format!(
            "<svg version='1.1' baseProfile='full' \
                  width='{}' height='{}' viewbox='-20 -20 {} {}' \
                  xmlns='http://www.w3.org/2000/svg'>",
            svg_w,
            svg_h,
            svg_w + 20,
            svg_h + 20
        );

        // Definitions.
        write!(&mut svg, "\
            <defs>\
              <marker class='svg-event-shape' id='arrow' markerWidth='12' markerHeight='10' refX='12' refY='5' orient='auto'>\
                <polygon points='0 0, 12 5, 0 10' />\
              </marker>\
            </defs>").unwrap();

        // Vertical timeline for each actor.
        for (actor_index, actor_name) in actor_names.iter().enumerate() {
            let (x1, y1) = plot(actor_index, 0);
            let (x2, y2) = plot(actor_index, path.len());
            writeln!(
                &mut svg,
                "<line x1='{}' y1='{}' x2='{}' y2='{}' class='svg-actor-timeline' />",
                x1, y1, x2, y2
            )
            .unwrap();
            writeln!(
                &mut svg,
                "<text x='{}' y='{}' class='svg-actor-label'>{}</text>",
                x1, y1, actor_name,
            )
            .unwrap();
        }

        // Arrow for each delivery. Circle for other events.
        let mut send_time = HashMap::new();
        for (time, (state, action)) in path.clone().into_iter().enumerate() {
            let time = time + 1; // action is for the next step
            match action {
                Some(ActorModelAction::Deliver { src, dst: id, msg }) => {
                    let src_time = *send_time.get(&(src, id, msg.clone())).unwrap_or(&0);
                    let (x1, y1) = plot(src.into(), src_time);
                    let (x2, y2) = plot(id.into(), time);
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
                Some(ActorModelAction::Timeout(actor_id, timer)) => {
                    let (x, y) = plot(actor_id.into(), time);
                    writeln!(
                        &mut svg,
                        "<circle cx='{}' cy='{}' r='10' class='svg-event-shape' />",
                        x, y
                    )
                    .unwrap();

                    // Track sends to facilitate building arrows.
                    let index = usize::from(actor_id);
                    if let Some(actor_state) = state.actor_states.get(index) {
                        let mut actor_state = Cow::Borrowed(&**actor_state);
                        let mut out = Out::new();
                        self.actors[index].on_timeout(actor_id, &mut actor_state, &timer, &mut out);
                        for command in out {
                            if let Command::Send(dst, msg) = command {
                                send_time.insert((actor_id, dst, msg), time);
                            }
                        }
                    }
                }
                Some(ActorModelAction::Crash(actor_id)) => {
                    let (x, y) = plot(actor_id.into(), time);
                    writeln!(
                        &mut svg,
                        "<circle cx='{}' cy='{}' r='10' class='svg-event-shape' />",
                        x, y
                    )
                    .unwrap();
                }
                Some(ActorModelAction::SelectRandom {
                    actor,
                    key: _,
                    random,
                }) => {
                    let (x, y) = plot(actor.into(), time);
                    writeln!(
                        &mut svg,
                        "<circle cx='{}' cy='{}' r='10' class='svg-event-shape' />",
                        x, y
                    )
                    .unwrap();

                    // Track sends to facilitate building arrows.
                    let index = usize::from(actor);
                    if let Some(actor_state) = state.actor_states.get(index) {
                        let mut actor_state = Cow::Borrowed(&**actor_state);
                        let mut out = Out::new();
                        self.actors[index].on_random(actor, &mut actor_state, &random, &mut out);
                        for command in out {
                            if let Command::Send(dst, msg) = command {
                                send_time.insert((actor, dst, msg), time);
                            }
                        }
                    }
                }
                Some(ActorModelAction::Recover(actor_id)) => {
                    let (x, y) = plot(actor_id.into(), time);
                    writeln!(
                        &mut svg,
                        "<circle cx='{}' cy='{}' r='10' class='svg-event-shape' />",
                        x, y
                    )
                    .unwrap();
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
                    writeln!(
                        &mut svg,
                        "<text x='{}' y='{}' class='svg-event-label'>{:?}</text>",
                        x, y, msg
                    )
                    .unwrap();
                }
                Some(ActorModelAction::Timeout(id, timer)) => {
                    let (x, y) = plot(id.into(), time);
                    writeln!(
                        &mut svg,
                        "<text x='{}' y='{}' class='svg-event-label'>Timeout({:?})</text>",
                        x, y, timer
                    )
                    .unwrap();
                }
                Some(ActorModelAction::Crash(id)) => {
                    let (x, y) = plot(id.into(), time);
                    writeln!(
                        &mut svg,
                        "<text x='{}' y='{}' class='svg-event-label'>Crash</text>",
                        x, y
                    )
                    .unwrap();
                }
                Some(ActorModelAction::SelectRandom {
                    actor,
                    key: _,
                    random,
                }) => {
                    let (x, y) = plot(actor.into(), time);
                    writeln!(
                        &mut svg,
                        "<text x='{}' y='{}' class='svg-event-label'>Random({:?})</text>",
                        x, y, random
                    )
                    .unwrap();
                }
                Some(ActorModelAction::Recover(id)) => {
                    let (x, y) = plot(id.into(), time);
                    writeln!(
                        &mut svg,
                        "<text x='{}' y='{}' class='svg-event-label'>Recover</text>",
                        x, y
                    )
                    .unwrap();
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
    use crate::actor::actor_test_util::ping_pong::{PingPongCfg, PingPongMsg, PingPongMsg::*};
    use crate::actor::ActorModelAction::*;
    use crate::{Checker, PathRecorder, StateRecorder};
    use std::collections::{BTreeSet, HashSet};
    use std::sync::Arc;

    #[test]
    fn visits_expected_states() {
        use std::iter::FromIterator;

        // helper to make the test more concise
        let states_and_network =
            |states: Vec<u32>,
             envelopes: Vec<Envelope<_>>,
             last_msg: Option<Envelope<PingPongMsg>>| {
                let states_len = states.len();
                let timers_set = vec![Timers::new(); states_len];
                let crashed = vec![false; states.len()];
                ActorModelState {
                    actor_states: states.into_iter().map(Arc::new).collect::<Vec<_>>(),
                    network: Network::new_unordered_duplicating_with_last_msg(envelopes, last_msg),
                    timers_set,
                    random_choices: vec![RandomChoices::default(); states_len],
                    crashed,
                    history: (0_u32, 0_u32), // constant as `maintains_history: false`
                    actor_storages: vec![None; states_len],
                }
            };

        let (recorder, accessor) = StateRecorder::new_with_accessor();
        let checker = PingPongCfg {
            maintains_history: false,
            max_nat: 1,
        }
        .into_model()
        .lossy_network(LossyNetwork::Yes)
        .checker()
        .visitor(recorder)
        .spawn_bfs()
        .join();
        assert_eq!(checker.unique_state_count(), 14);

        let state_space = accessor();
        assert_eq!(state_space.len(), 14); // same as the generated count
        #[rustfmt::skip]
        assert_eq!(HashSet::<_>::from_iter(state_space), HashSet::from_iter(vec![
            // When the network loses no messages...
            states_and_network(
                vec![0, 0],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) }],
                None),
            states_and_network(
                vec![0, 1],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                ],
                Some(Envelope{ src: Id::from(0), dst: Id::from(1), msg: Ping(0) })),
            states_and_network(
                vec![1, 1],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) },
                ],
                Some(Envelope{ src: Id::from(1), dst: Id::from(0), msg: Pong(0) })),

            // When the network loses the message for pinger-ponger state (0, 0)...
            states_and_network(
                vec![0, 0],
                Vec::new(),
                None),

            // When the network loses a message for pinger-ponger state (0, 1)
            states_and_network(
                vec![0, 1],
                vec![Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) }],
                Some(Envelope{ src: Id::from(0), dst: Id::from(1), msg: Ping(0) })),
            states_and_network(
                vec![0, 1],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) }],
                Some(Envelope{ src: Id::from(0), dst: Id::from(1), msg: Ping(0) })),
            states_and_network(
                vec![0, 1],
                Vec::new(),
                Some(Envelope{ src: Id::from(0), dst: Id::from(1), msg: Ping(0) })),

            // When the network loses a message for pinger-ponger state (1, 1)
            states_and_network(
                vec![1, 1],
                vec![
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) },
                ],
                Some(Envelope{ src: Id::from(1), dst: Id::from(0), msg: Pong(0) })),
            states_and_network(
                vec![1, 1],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) },
                ],
                Some(Envelope{ src: Id::from(1), dst: Id::from(0), msg: Pong(0) })),
            states_and_network(
                vec![1, 1],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                ],
                Some(Envelope{ src: Id::from(1), dst: Id::from(0), msg: Pong(0) })),
            states_and_network(
                vec![1, 1],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) }],
                Some(Envelope{ src: Id::from(1), dst: Id::from(0), msg: Pong(0) })),
            states_and_network(
                vec![1, 1],
                vec![Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) }],
                Some(Envelope{ src: Id::from(1), dst: Id::from(0), msg: Pong(0) })),
            states_and_network(
                vec![1, 1],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) }],
                Some(Envelope{ src: Id::from(1), dst: Id::from(0), msg: Pong(0) })),
            states_and_network(
                vec![1, 1],
                Vec::new(),
                Some(Envelope{ src: Id::from(1), dst: Id::from(0), msg: Pong(0) })),
        ]));
    }

    #[test]
    fn no_op_depends_on_network() {
        #[derive(Clone)]
        enum MyActor {
            Client { server: Id },
            Server,
        }
        #[derive(Clone, Debug, Eq, Hash, PartialEq)]
        enum Msg {
            Ignored,
            Interesting,
        }
        impl Actor for MyActor {
            type Msg = Msg;
            type State = String;
            type Timer = ();
            type Random = ();
            type Storage = ();
            fn on_start(
                &self,
                _id: Id,
                _storage: &Option<Self::Storage>,
                o: &mut Out<Self>,
            ) -> Self::State {
                if let MyActor::Client { server } = self {
                    o.send(*server, Msg::Ignored);
                    o.send(*server, Msg::Interesting);
                }
                "Awaiting an interesting message.".to_string()
            }
            fn on_msg(
                &self,
                _: Id,
                state: &mut Cow<Self::State>,
                _: Id,
                msg: Self::Msg,
                _: &mut Out<Self>,
            ) {
                if msg == Msg::Interesting {
                    *state.to_mut() = "Got an interesting message.".to_string();
                }
            }
        }

        let model = ActorModel::new((), ())
            .actor(MyActor::Client { server: 1.into() })
            .actor(MyActor::Server)
            .lossy_network(LossyNetwork::No)
            .property(Expectation::Always, "Check everything", |_, _| true);
        assert_eq!(
            model
                .clone()
                .init_network(Network::new_unordered_duplicating([]))
                .checker()
                .spawn_bfs()
                .join()
                .unique_state_count(),
            2 // initial and delivery of Interesting
        );
        assert_eq!(
            model
                .clone()
                .init_network(Network::new_unordered_nonduplicating([]))
                .checker()
                .spawn_bfs()
                .join()
                .unique_state_count(),
            2 // initial and delivery of Interesting
        );
        assert_eq!(
            model
                .clone()
                .init_network(Network::new_ordered([]))
                .checker()
                .spawn_bfs()
                .join()
                .unique_state_count(),
            3 // initial, delivery of Uninteresting, and subsequent delivery of Interesting
        );
    }

    #[test]
    fn maintains_fixed_delta_despite_lossy_duplicating_network() {
        let checker = PingPongCfg {
            max_nat: 5,
            maintains_history: false,
        }
        .into_model()
        .lossy_network(LossyNetwork::Yes)
        .checker()
        .spawn_bfs()
        .join();
        assert_eq!(checker.unique_state_count(), 4_094);
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
        .checker()
        .spawn_bfs()
        .join();
        assert_eq!(checker.unique_state_count(), 4_094);

        // can lose the first message and get stuck, for example
        checker.assert_discovery(
            "must reach max",
            vec![Drop(Envelope {
                src: Id(0),
                dst: Id(1),
                msg: Ping(0),
            })],
        );
    }

    #[test]
    fn eventually_reaches_max_on_perfect_delivery_network() {
        let checker = PingPongCfg {
            max_nat: 5,
            maintains_history: false,
        }
        .into_model()
        .init_network(Network::new_unordered_nonduplicating([]))
        .lossy_network(LossyNetwork::No)
        .checker()
        .spawn_bfs()
        .join();
        assert_eq!(checker.unique_state_count(), 11);
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
        .checker()
        .spawn_bfs()
        .join();
        assert_eq!(checker.unique_state_count(), 11);
        assert_eq!(
            checker
                .discovery("can reach max")
                .unwrap()
                .last_state()
                .actor_states,
            vec![Arc::new(4), Arc::new(5)]
        );
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
        .init_network(Network::new_unordered_nonduplicating([]))
        .lossy_network(LossyNetwork::No)
        .checker()
        .spawn_bfs()
        .join();
        assert_eq!(checker.unique_state_count(), 11);

        // this is an example of a liveness property that fails to hold (due to the boundary)
        assert_eq!(
            checker
                .discovery("must exceed max")
                .unwrap()
                .last_state()
                .actor_states,
            vec![Arc::new(5), Arc::new(5)]
        );
    }

    #[test]
    fn handles_undeliverable_messages() {
        assert_eq!(
            ActorModel::new((), ())
                .actor(())
                .property(Expectation::Always, "unused", |_, _| true) // force full traversal
                .init_network(Network::new_unordered_duplicating([Envelope {
                    src: 0.into(),
                    dst: 99.into(),
                    msg: ()
                }]))
                .checker()
                .spawn_bfs()
                .join()
                .unique_state_count(),
            1
        );
    }

    #[test]
    fn handles_ordered_network_flag() {
        #[derive(Clone)]
        struct OrderedNetworkActor;
        impl Actor for OrderedNetworkActor {
            type Msg = u8;
            type State = Vec<u8>;
            type Timer = ();
            type Random = ();
            type Storage = ();
            fn on_start(
                &self,
                id: Id,
                _storage: &Option<Self::Storage>,
                o: &mut Out<Self>,
            ) -> Self::State {
                if id == 0.into() {
                    // Count down.
                    o.send(1.into(), 2);
                    o.send(1.into(), 1);
                }
                Vec::new()
            }
            fn on_msg(
                &self,
                _: Id,
                state: &mut Cow<Self::State>,
                _: Id,
                msg: Self::Msg,
                _: &mut Out<Self>,
            ) {
                state.to_mut().push(msg);
            }
        }

        let model = ActorModel::new((), ())
            .actors(vec![OrderedNetworkActor, OrderedNetworkActor])
            .property(Expectation::Always, "", |_, _| true)
            .init_network(Network::new_unordered_nonduplicating([]));

        // Fewer states if network is ordered.
        let (recorder, accessor) = StateRecorder::new_with_accessor();
        model
            .clone()
            .init_network(Network::new_ordered([]))
            .checker()
            .visitor(recorder)
            .spawn_bfs()
            .join();
        let recipient_states: BTreeSet<Vec<u8>> = accessor()
            .into_iter()
            .map(|s| (*s.actor_states[1]).clone())
            .collect();
        assert_eq!(
            recipient_states,
            BTreeSet::from([vec![], vec![2], vec![2, 1]])
        );

        // More states if network is not ordered.
        let (recorder, accessor) = StateRecorder::new_with_accessor();
        model
            .init_network(Network::new_unordered_nonduplicating([]))
            .checker()
            .visitor(recorder)
            .spawn_bfs()
            .join();
        let recipient_states: BTreeSet<Vec<u8>> = accessor()
            .into_iter()
            .map(|s| (*s.actor_states[1]).clone())
            .collect();
        assert_eq!(
            recipient_states,
            BTreeSet::from([vec![], vec![1], vec![2], vec![1, 2], vec![2, 1]]),
        );
    }

    #[test]
    fn unordered_network_has_a_bug() {
        // Imagine that actor 1 sends two copies of a message to actor 2. An earlier implementation
        // used a set to track envelopes in an unordered network even if it was lossy, and
        // therefore could not distinguish between dropping/delivering 1 message vs multiple
        // pending copies of the same message.

        fn enumerate_action_sequences(
            lossy: LossyNetwork,
            init_network: Network<()>,
        ) -> HashSet<Vec<ActorModelAction<(), (), ()>>> {
            // There are two actors, and the first sends the same two messages to the second, which
            // counts them.
            struct A;
            impl Actor for A {
                type Msg = ();
                type State = usize; // receipt count
                type Timer = ();
                type Random = ();
                type Storage = ();

                fn on_start(
                    &self,
                    id: Id,
                    _storage: &Option<Self::Storage>,
                    o: &mut Out<Self>,
                ) -> Self::State {
                    if id == 0.into() {
                        o.send(1.into(), ());
                        o.send(1.into(), ());
                    }
                    0
                }
                fn on_msg(
                    &self,
                    _: Id,
                    state: &mut Cow<Self::State>,
                    _: Id,
                    _: Self::Msg,
                    _: &mut Out<Self>,
                ) {
                    *state.to_mut() += 1;
                }
            }

            // Return the actions taken for each path based on the specified network
            // characteristics.
            let (recorder, accessor) = PathRecorder::new_with_accessor();
            ActorModel::new((), ())
                .actors([A, A])
                .init_network(init_network)
                .lossy_network(lossy)
                .property(Expectation::Always, "force visiting all states", |_, _| {
                    true
                })
                .within_boundary(|_, s| *s.actor_states[1] < 4)
                .checker()
                .visitor(recorder)
                .spawn_dfs()
                .join();
            accessor().into_iter().map(|p| p.into_actions()).collect()
        }

        // The actions are named here for brevity. These implement `Copy`.
        let deliver = ActorModelAction::<(), (), ()>::Deliver {
            src: 0.into(),
            msg: (),
            dst: 1.into(),
        };
        let drop = ActorModelAction::<(), (), ()>::Drop(Envelope {
            src: 0.into(),
            msg: (),
            dst: 1.into(),
        });

        // Ordered networks can deliver/drop both messages.
        let ordered_lossless =
            enumerate_action_sequences(LossyNetwork::No, Network::new_ordered([]));
        assert!(ordered_lossless.contains(&vec![deliver.clone(), deliver.clone()]));
        assert!(!ordered_lossless.contains(&vec![
            deliver.clone(),
            deliver.clone(),
            deliver.clone()
        ]));
        let ordered_lossy = enumerate_action_sequences(LossyNetwork::Yes, Network::new_ordered([]));
        assert!(ordered_lossy.contains(&vec![deliver.clone(), deliver.clone()]));
        assert!(ordered_lossy.contains(&vec![deliver.clone(), drop.clone()])); // same state as "drop, deliver"
        assert!(ordered_lossy.contains(&vec![drop.clone(), drop.clone()]));

        // Unordered duplicating networks can deliver/drop duplicates.
        //
        // IMPORTANT: in the context of a duplicating network, "dropping" must either entail:
        //            (1) a no-op or (2) never delivering again. This implementation favors the
        //            latter.
        let unord_dup_lossless =
            enumerate_action_sequences(LossyNetwork::No, Network::new_unordered_duplicating([]));
        assert!(unord_dup_lossless.contains(&vec![
            deliver.clone(),
            deliver.clone(),
            deliver.clone()
        ]));
        let unord_dup_lossy =
            enumerate_action_sequences(LossyNetwork::Yes, Network::new_unordered_duplicating([]));
        assert!(unord_dup_lossy.contains(&vec![deliver.clone(), deliver.clone(), deliver.clone()]));
        assert!(unord_dup_lossy.contains(&vec![deliver.clone(), deliver.clone(), drop.clone()]));
        assert!(unord_dup_lossy.contains(&vec![deliver.clone(), drop.clone()]));
        assert!(unord_dup_lossy.contains(&vec![drop.clone()]));
        assert!(!unord_dup_lossy.contains(&vec![drop.clone(), deliver.clone()])); // b/c drop means "never deliver again"

        // Unordered nonduplicating networks can deliver/drop both messages.
        let unord_nondup_lossless =
            enumerate_action_sequences(LossyNetwork::No, Network::new_unordered_nonduplicating([]));
        assert!(unord_nondup_lossless.contains(&vec![deliver.clone(), deliver.clone()]));
        let unord_nondup_lossy = enumerate_action_sequences(
            LossyNetwork::Yes,
            Network::new_unordered_nonduplicating([]),
        );
        assert!(unord_nondup_lossy.contains(&vec![deliver.clone(), drop.clone()]));
        assert!(unord_nondup_lossy.contains(&vec![drop.clone(), drop.clone()]));
    }

    #[test]
    fn resets_timer() {
        struct TestActor;
        impl Actor for TestActor {
            type State = ();
            type Msg = ();
            type Timer = ();
            type Random = ();
            type Storage = ();
            fn on_start(&self, _: Id, _: &Option<Self::Storage>, o: &mut Out<Self>) {
                o.set_timer((), model_timeout());
            }
            fn on_msg(
                &self,
                _: Id,
                _: &mut Cow<Self::State>,
                _: Id,
                _: Self::Msg,
                _: &mut Out<Self>,
            ) {
            }
        }

        // Init state with timer, followed by next state without timer.
        assert_eq!(
            ActorModel::new((), ())
                .actor(TestActor)
                .property(Expectation::Always, "unused", |_, _| true) // force full traversal
                .checker()
                .spawn_bfs()
                .join()
                .unique_state_count(),
            2
        );
    }

    #[test]
    fn choose_random() {
        #[derive(Hash, PartialEq, Eq, Debug, Clone)]
        enum TestRandom {
            Choice1,
            Choice2,
            Choice3,
        }

        struct TestActor;
        impl Actor for TestActor {
            type State = Option<TestRandom>;
            type Msg = ();
            type Timer = ();
            type Random = TestRandom;
            type Storage = ();
            fn on_start(
                &self,
                _: Id,
                _: &Option<Self::Storage>,
                o: &mut Out<Self>,
            ) -> Option<TestRandom> {
                o.choose_random(
                    "key1",
                    vec![
                        TestRandom::Choice1,
                        TestRandom::Choice2,
                        TestRandom::Choice3,
                    ],
                );
                None
            }
            fn on_msg(
                &self,
                _: Id,
                _: &mut Cow<Self::State>,
                _: Id,
                _: Self::Msg,
                _: &mut Out<Self>,
            ) {
            }
            fn on_random(
                &self,
                _: Id,
                state: &mut Cow<Self::State>,
                random: &Self::Random,
                _: &mut Out<Self>,
            ) {
                *state.to_mut() = Some(random.clone());
            }
        }

        // Init state with a random choice, followed by 3 possible next states.
        assert_eq!(
            ActorModel::new((), ())
                .actor(TestActor)
                .property(Expectation::Always, "unused", |_, _| true) // force full traversal
                .checker()
                .spawn_bfs()
                .join()
                .unique_state_count(),
            4
        );
    }

    #[test]
    fn overwrite_choose_random() {
        #[derive(Hash, PartialEq, Eq, Debug, Clone)]
        enum TestRandom {
            Choice1,
            Choice2,
            Choice3,
        }

        struct TestActor;
        impl Actor for TestActor {
            type State = Vec<TestRandom>;
            type Msg = ();
            type Timer = ();
            type Random = TestRandom;
            type Storage = ();
            fn on_start(
                &self,
                _: Id,
                _: &Option<Self::Storage>,
                o: &mut Out<Self>,
            ) -> Vec<TestRandom> {
                o.choose_random("key1", vec![TestRandom::Choice1]);
                o.choose_random("key2", vec![TestRandom::Choice2, TestRandom::Choice3]);
                Vec::new()
            }
            fn on_msg(
                &self,
                _: Id,
                _: &mut Cow<Self::State>,
                _: Id,
                _: Self::Msg,
                _: &mut Out<Self>,
            ) {
            }
            fn on_random(
                &self,
                _: Id,
                state: &mut Cow<Self::State>,
                random: &Self::Random,
                o: &mut Out<Self>,
            ) {
                if *random == TestRandom::Choice1 {
                    // If choice1 is chosen, set "key2" to just Choice3.
                    // If "key2" hasn't been taken yet, this will reduce the choice from [2,3] to
                    //    just [3].
                    // If "key2" has already been taken, this will allow the actor to choose "key2"
                    //    again, this time with just one option ([Choice3]).
                    o.choose_random("key2", vec![TestRandom::Choice3]);
                }
                state.to_mut().push(random.clone());
            }
        }

        //      /-> key1:Choice1 -> key2:Choice3
        // Init --> key2:Choice2 -> key1:Choice1 -> key2:Choice3
        //      \-> key2:Choice3 -> key1:Choice1 -> key2:Choice3
        assert_eq!(
            ActorModel::new((), ())
                .actor(TestActor)
                .property(Expectation::Always, "unused", |_, _| true) // force full traversal
                .checker()
                .spawn_bfs()
                .join()
                .unique_state_count(),
            9
        );
    }

    #[test]
    fn crash_test() {
        #[derive(Clone)]
        struct TestActor;
        impl Actor for TestActor {
            type Msg = ();
            type Timer = ();
            type State = ();
            type Storage = ();
            type Random = ();
            fn on_start(
                &self,
                _id: Id,
                _storage: &Option<Self::Storage>,
                _o: &mut Out<Self>,
            ) -> Self::State {
                ()
            }
        }

        assert_eq!(
            ActorModel::new((), ())
                .actors(vec![TestActor; 3])
                .max_crashes(1)
                .property(Expectation::Always, "unused", |_, _| true) // force full traversal
                .checker()
                .spawn_bfs()
                .join()
                .unique_state_count(),
            // comb(3, 1) + 1 (no actor is crashed) = 3 + 1 = 4
            4
        )
    }

    #[test]
    fn recover_test() {
        #[derive(Clone)]
        struct TestActor(usize); // carry a counter to limit the model space
        #[derive(Clone, Debug, PartialEq, Hash)]
        struct TestState {
            volatile: usize,
            non_volatile: usize,
        }
        #[derive(Clone, Debug, PartialEq, Hash)]
        struct TestStorage {
            non_volatile: usize,
        }

        impl Actor for TestActor {
            type Msg = ();
            type Timer = ();
            type State = TestState;
            type Storage = TestStorage;
            type Random = ();

            fn on_start(
                &self,
                _id: Id,
                storage: &Option<Self::Storage>,
                o: &mut Out<Self>,
            ) -> Self::State {
                o.set_timer((), model_timeout());
                if let Some(storage) = storage {
                    Self::State {
                        volatile: 0,
                        non_volatile: storage.non_volatile, // restore non-volatile state from `storage`
                    }
                } else {
                    Self::State {
                        volatile: 0,
                        non_volatile: 0, // no available `storage`
                    }
                }
            }

            fn on_timeout(
                &self,
                _id: Id,
                state: &mut Cow<Self::State>,
                _timer: &Self::Timer,
                o: &mut Out<Self>,
            ) {
                let state = state.to_mut();
                state.volatile += 1;
                state.non_volatile += 1;
                o.save(TestStorage {
                    non_volatile: state.non_volatile,
                });
                if state.non_volatile < self.0 {
                    // restore the timer
                    o.set_timer((), model_timeout());
                }
            }
        }

        let checker = ActorModel::new((), ())
            .actor(TestActor(3))
            .property(Expectation::Sometimes, "recovered", |_, state| {
                let actor_state = state.actor_states.get(0).unwrap();
                actor_state.non_volatile > actor_state.volatile
            })
            .max_crashes(1)
            .checker()
            .spawn_bfs()
            .join();
        checker.assert_any_discovery("recovered");
    }
}

#[cfg(test)]
mod choice_test {
    use super::*;
    use crate::{Checker, Model, StateRecorder};
    use choice::Choice;

    #[derive(Clone)]
    struct A {
        b: Id,
    }
    impl Actor for A {
        type State = u8;
        type Msg = ();
        type Timer = ();
        type Random = ();
        type Storage = ();
        fn on_start(&self, _: Id, _: &Option<Self::Storage>, _: &mut Out<Self>) -> Self::State {
            1
        }
        fn on_msg(
            &self,
            _: Id,
            state: &mut Cow<Self::State>,
            _: Id,
            _: Self::Msg,
            o: &mut Out<Self>,
        ) {
            *state.to_mut() = state.wrapping_add(1);
            o.send(self.b, ());
        }
    }

    #[derive(Clone)]
    struct B {
        c: Id,
    }
    impl Actor for B {
        type State = char;
        type Msg = ();
        type Timer = ();
        type Random = ();
        type Storage = ();
        fn on_start(&self, _: Id, _: &Option<Self::Storage>, _: &mut Out<Self>) -> Self::State {
            'a'
        }
        fn on_msg(
            &self,
            _: Id,
            state: &mut Cow<Self::State>,
            _: Id,
            _: Self::Msg,
            o: &mut Out<Self>,
        ) {
            *state.to_mut() = (**state as u8).wrapping_add(1) as char;
            o.send(self.c, ());
        }
    }

    #[derive(Clone)]
    struct C {
        a: Id,
    }
    impl Actor for C {
        type State = String;
        type Msg = ();
        type Timer = ();
        type Random = ();
        type Storage = ();
        fn on_start(&self, _: Id, _: &Option<Self::Storage>, o: &mut Out<Self>) -> Self::State {
            o.send(self.a, ());
            "I".to_string()
        }
        fn on_msg(
            &self,
            _: Id,
            state: &mut Cow<Self::State>,
            _: Id,
            _: Self::Msg,
            o: &mut Out<Self>,
        ) {
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
            .init_network(Network::new_unordered_nonduplicating([]))
            .record_msg_out(|_, out_count, _| Some(out_count + 1))
            .property(Expectation::Always, "true", |_, _| true)
            .within_boundary(|_, state| state.history < 8);

        let (recorder, accessor) = StateRecorder::new_with_accessor();

        sys.checker().visitor(recorder).spawn_dfs().join();

        type StateVector = choice![u8, char, String];
        let states: Vec<Vec<StateVector>> = accessor()
            .into_iter()
            .map(|s| s.actor_states.into_iter().map(|a| (*a).clone()).collect())
            .collect();
        assert_eq!(
            states,
            vec![
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
            ]
        );
    }
}
