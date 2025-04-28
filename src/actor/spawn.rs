//! Private module for selective re-export.

use crate::actor::*;
use crossbeam_utils::thread;
use std::collections::HashMap;
use std::fmt::Debug;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

impl From<Id> for SocketAddrV4 {
    fn from(id: Id) -> Self {
        let bytes = id.0.to_be_bytes();
        let ip = Ipv4Addr::from([bytes[2], bytes[3], bytes[4], bytes[5]]);
        let port = u16::from_be_bytes([bytes[6], bytes[7]]);
        SocketAddrV4::new(ip, port)
    }
}

impl From<SocketAddrV4> for Id {
    fn from(addr: SocketAddrV4) -> Self {
        let octets = addr.ip().octets();
        let port_bytes = addr.port().to_be_bytes();
        let mut result: [u8; 8] = [0; 8];
        result[0] = 0;
        result[1] = 0;
        result[2] = octets[0];
        result[3] = octets[1];
        result[4] = octets[2];
        result[5] = octets[3];
        result[6] = port_bytes[0];
        result[7] = port_bytes[1];
        Id(u64::from_be_bytes(result))
    }
}

/// 500 years in the future.
fn practically_never() -> Instant {
    Instant::now() + Duration::from_secs(3600 * 24 * 365 * 500)
}

/// Runs an actor, sending messages over UDP. Blocks the current thread.
///
/// # Example
///
/// ```no_run
/// use stateright::actor::{Id, spawn};
/// use std::net::{Ipv4Addr, SocketAddrV4};
/// # mod serde_json {
/// #     pub fn to_vec(_: &()) -> Result<Vec<u8>, ()> { Ok(vec![]) }
/// #     pub fn from_slice(_: &[u8]) -> Result<(), ()> { Ok(()) }
/// # }
/// # let actor1 = ();
/// # let actor2 = ();
/// let id1 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3001));
/// let id2 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3002));
/// spawn(
///     serde_json::to_vec,
///     |bytes| serde_json::from_slice(bytes),
///     serde_json::to_vec,
///     |bytes| serde_json::from_slice(bytes),
///     vec![
///         (id1, actor1),
///         (id2, actor2),
///     ]);
/// ```
pub fn spawn<A, E: Debug + 'static>(
    msg_serialize: fn(&A::Msg) -> Result<Vec<u8>, E>,
    msg_deserialize: fn(&[u8]) -> Result<A::Msg, E>,
    storage_serialize: fn(&A::Storage) -> Result<Vec<u8>, E>,
    storage_deserialize: fn(&[u8]) -> Result<A::Storage, E>,
    actors: Vec<(impl Into<Id>, A)>,
) -> Result<(), Box<dyn std::any::Any + Send + 'static>>
where
    A: 'static + Send + Actor,
    A::Msg: Debug,
    A::State: Debug,
    A::Storage: Debug,
{
    thread::scope(|s| {
        for (id, actor) in actors {
            let id = id.into();
            let addr = SocketAddrV4::from(id);

            // note that panics are returned as `Err` when `join`ing
            s.spawn(move |_| {
                let socket = UdpSocket::bind(addr).unwrap(); // panic if unable to bind
                let mut in_buf = [0; 65_535];
                let mut next_interrupts = HashMap::new();

                let mut out = Out::new();
                let filename = format!("{}.storage", addr);
                let path = PathBuf::from(filename);
                let storage: Option<A::Storage> = fs::read(&path)
                    .ok()
                    .and_then(|bytes| storage_deserialize(&bytes).ok());
                let mut state = Cow::Owned(actor.on_start(id, &storage, &mut out));
                log::info!("Actor started. id={}, state={:?}, out={:?}", addr, state, out);
                for c in out {
                    on_command::<A, E>(addr, c, msg_serialize, storage_serialize, &socket, &mut next_interrupts);
                }

                loop {
                    // Apply an interrupt if present, otherwise wait for a message.
                    let mut out = Out::new();
                    let (min_timer, min_instant) = next_interrupts
                        .iter()
                        .min_by_key(|(_, instant)| *instant)
                        .map(|(t, i)| (Some(t.clone()), *i))
                        .unwrap_or_else(|| (None, practically_never()));
                    if let Some(max_wait) = min_instant.checked_duration_since(Instant::now()) {
                        socket.set_read_timeout(Some(max_wait)).expect("set_read_timeout failed");
                        match socket.recv_from(&mut in_buf) {
                            Err(e) => {
                                // Timeout (`WouldBlock`) ignored since next iteration will apply interrupt.
                                if e.kind() != std::io::ErrorKind::WouldBlock {
                                    log::warn!("Unable to read socket. Ignoring. id={}, err={:?}", addr, e);
                                }
                                continue;
                            },
                            Ok((count, src_addr)) => {
                                match msg_deserialize(&in_buf[..count]) {
                                    Ok(msg) => {
                                        if let SocketAddr::V4(src_addr) = src_addr {
                                            log::info!("Received message. id={}, src={}, msg={}",
                                                        addr, src_addr, format!("{:?}", msg));
                                            actor.on_msg(id, &mut state, Id::from(src_addr), msg, &mut out);
                                        } else {
                                            log::debug!("Received non-IPv4 message. Ignoring. id={}, src={}, msg={}",
                                                       addr, src_addr, format!("{:?}", msg));
                                            continue;
                                        }
                                    },
                                    Err(e) => {
                                        log::debug!("Unable to parse message. Ignoring. id={}, src={}, buf={:?}, err={:?}",
                                                   addr, src_addr, &in_buf[..count], e);
                                        continue;
                                    }
                                }
                            },
                        }
                    } else {
                        let min_timer = min_timer.unwrap();
                        next_interrupts.remove(&min_timer); // timer is no longer valid
                        match &min_timer {
                            Interrupt::Timeout(min_timer) => {
                                actor.on_timeout(id, &mut state, min_timer, &mut out);
                            },
                            Interrupt::Random(random) => {
                                actor.on_random(id, &mut state, random, &mut out);
                            },
                        }
                    }

                    // Handle commands and update state.
                    if !is_no_op(&state, &out) {
                        log::debug!("Acted. id={}, state={:?}, out={:?}",
                                    addr, state, out);
                    }
                    for c in out { on_command::<A, E>(addr, c, msg_serialize, storage_serialize, &socket, &mut next_interrupts); }
                }
            });
        }
    })
}

#[derive(Hash, PartialEq, Eq, Clone)]
enum Interrupt<T, R> {
    Timeout(T),
    Random(R),
}

/// The effect to perform in response to spawned actor outputs.
fn on_command<A, E>(
    addr: SocketAddrV4,
    command: Command<A::Msg, A::Timer, A::Random, A::Storage>,
    msg_serialize: fn(&A::Msg) -> Result<Vec<u8>, E>,
    storage_serialize: fn(&A::Storage) -> Result<Vec<u8>, E>,
    socket: &UdpSocket,
    next_interrupts: &mut HashMap<Interrupt<A::Timer, A::Random>, Instant>,
) where
    A: Actor,
    A::Msg: Debug,
    E: Debug,
{
    match command {
        Command::Send(dst, msg) => {
            let dst_addr = SocketAddrV4::from(dst);
            match msg_serialize(&msg) {
                Err(e) => {
                    log::warn!(
                        "Unable to serialize. Ignoring. src={}, dst={}, msg={:?}, err={:?}",
                        addr,
                        dst_addr,
                        msg,
                        e
                    );
                }
                Ok(out_buf) => {
                    if let Err(e) = socket.send_to(&out_buf, dst_addr) {
                        log::warn!(
                            "Unable to send. Ignoring. src={}, dst={}, msg={:?}, err={:?}",
                            addr,
                            dst_addr,
                            msg,
                            e
                        );
                    }
                }
            }
        }
        Command::SetTimer(timer, range) => {
            let duration = if range.start < range.end {
                use rand::Rng;
                rand::thread_rng().gen_range(range.start..range.end)
            } else {
                range.start
            };
            next_interrupts
                .entry(Interrupt::Timeout(timer))
                .and_modify(|d| *d = Instant::now() + duration)
                .or_insert_with(|| Instant::now() + duration);
        }
        Command::CancelTimer(timer) => {
            // if not already set then that's fine to leave
            next_interrupts
                .entry(Interrupt::Timeout(timer))
                .and_modify(|d| *d = practically_never());
        }
        Command::ChooseRandom(_key, random) => {
            use rand::prelude::{Rng, SliceRandom};
            if random.is_empty() {
                return;
            }
            let mut rng = rand::thread_rng();
            let duration = rng.gen_range(Duration::ZERO..Duration::from_secs(10));
            let chosen_random = random.choose(&mut rng).unwrap();
            next_interrupts
                .entry(Interrupt::Random(chosen_random.clone()))
                .and_modify(|d| *d = Instant::now() + duration)
                .or_insert_with(|| Instant::now() + duration);
        }
        Command::Save(storage) => {
            let filename = format!("{}.storage", addr);
            let path = PathBuf::from(filename);
            let bytes = storage_serialize(&storage).expect("serialize storage failed");
            let mut file =
                File::create(&path).expect(format!("failed to create file {:?}", path).as_str());
            file.write_all(&bytes)
                .expect(format!("failed to write to file {:?}", path).as_str());
        }
    }
}

#[cfg(test)]
mod test {
    use crate::actor::*;
    use serde::{Deserialize, Serialize};
    use std::fs;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::path::PathBuf;

    #[test]
    fn can_encode_id() {
        let addr = SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 5);
        assert_eq!(Id::from(addr).0.to_be_bytes(), [0, 0, 1, 2, 3, 4, 0, 5]);
    }

    #[test]
    fn can_decode_id() {
        let addr = SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 5);
        assert_eq!(SocketAddrV4::from(Id::from(addr)), addr);
    }

    #[test]
    fn can_crash_and_then_recover() {
        #[derive(Clone)]
        struct TestActor;
        #[derive(Clone, Debug, PartialEq, Hash)]
        struct TestState {
            volatile: usize,
            non_volatile: usize,
        }
        #[derive(Clone, Debug, PartialEq, Hash, Serialize, Deserialize)]
        struct TestStorage {
            non_volatile: usize,
        }

        #[derive(Clone, Debug, PartialEq, Hash, Eq)]
        enum TestTimer {
            Increase,
            Crash,
        }
        impl Actor for TestActor {
            type Msg = ();
            type Timer = TestTimer;
            type State = TestState;
            type Storage = TestStorage;
            type Random = ();

            fn on_start(
                &self,
                _id: Id,
                storage: &Option<Self::Storage>,
                o: &mut Out<Self>,
            ) -> Self::State {
                o.set_timer(
                    TestTimer::Crash,
                    Duration::from_secs(2)..Duration::from_secs(3),
                );
                o.set_timer(
                    TestTimer::Increase,
                    Duration::from_millis(50)..Duration::from_millis(100),
                );
                if let Some(storage) = storage {
                    assert!(storage.non_volatile > 0);
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
                timer: &Self::Timer,
                o: &mut Out<Self>,
            ) {
                match timer {
                    TestTimer::Increase => {
                        let state = state.to_mut();
                        state.volatile += 1;
                        state.non_volatile += 1;
                        o.save(TestStorage {
                            non_volatile: state.non_volatile,
                        });
                        o.set_timer(
                            TestTimer::Increase,
                            Duration::from_millis(50)..Duration::from_millis(100),
                        );
                    }
                    TestTimer::Crash => {
                        panic!("Actor crashed!");
                    }
                }
            }
        }

        let id = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1234));
        let result = std::panic::catch_unwind(|| {
            spawn(
                serde_json::to_vec,
                |bytes| serde_json::from_slice(bytes),
                serde_json::to_vec,
                |bytes| serde_json::from_slice(bytes),
                vec![(id, TestActor)],
            )
        });
        if result.is_err() {
            // restart the actor
            let _ = std::panic::catch_unwind(|| {
                spawn(
                    serde_json::to_vec,
                    |bytes| serde_json::from_slice(bytes),
                    serde_json::to_vec,
                    |bytes| serde_json::from_slice(bytes),
                    vec![(id, TestActor)],
                )
            });
        }
        // delete the storage file
        let filename = format!("{}.storage", SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1234));
        let path = PathBuf::from(filename);
        fs::remove_file(&path).expect(&format!("failed to remove file {:?}", path));
    }
}
