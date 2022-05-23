//! Private module for selective re-export.

use crate::actor::*;
use crossbeam_utils::thread;
use std::fmt::Debug;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
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
///     vec![
///         (id1, actor1),
///         (id2, actor2),
///     ]);
/// ```
pub fn spawn<A, E: Debug + 'static>(
    serialize: fn(&A::Msg) -> Result<Vec<u8>, E>,
    deserialize: fn(&[u8]) -> Result<A::Msg, E>,
    actors: Vec<(impl Into<Id>, A)>,
) -> Result<(), Box<dyn std::any::Any + Send + 'static>>
where
    A: 'static + Send + Actor,
    A::Msg: Debug,
    A::State: Debug,
{
    thread::scope(|s| {
        for (id, actor) in actors {
            let id = id.into();
            let addr = SocketAddrV4::from(id);

            // note that panics are returned as `Err` when `join`ing
            s.spawn(move |_| {
                let socket = UdpSocket::bind(addr).unwrap(); // panic if unable to bind
                let mut in_buf = [0; 65_535];
                let mut next_interrupt = practically_never();

                let mut out = Out::new();
                let mut state = Cow::Owned(actor.on_start(id, &mut out));
                log::info!("Actor started. id={}, state={:?}, out={:?}", addr, state, out);
                for c in out {
                    on_command::<A, E>(addr, c, serialize, &socket, &mut next_interrupt);
                }

                loop {
                    // Apply an interrupt if present, otherwise wait for a message.
                    let mut out = Out::new();
                    if let Some(max_wait) = next_interrupt.checked_duration_since(Instant::now()) {
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
                                match deserialize(&in_buf[..count]) {
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
                        next_interrupt = practically_never(); // timer is no longer valid
                        actor.on_timeout(id, &mut state, &mut out);
                    };

                    // Handle commands and update state.
                    if !is_no_op(&state, &out) {
                        log::debug!("Acted. id={}, state={:?}, out={:?}",
                                    addr, state, out);
                    }
                    for c in out { on_command::<A, E>(addr, c, serialize, &socket, &mut next_interrupt); }
                }
            });
        }
    })
}

/// The effect to perform in response to spawned actor outputs.
fn on_command<A, E>(
    addr: SocketAddrV4,
    command: Command<A::Msg>,
    serialize: fn(&A::Msg) -> Result<Vec<u8>, E>,
    socket: &UdpSocket,
    next_interrupt: &mut Instant,
) where
    A: Actor,
    A::Msg: Debug,
    E: Debug,
{
    match command {
        Command::Send(dst, msg) => {
            let dst_addr = SocketAddrV4::from(dst);
            match serialize(&msg) {
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
        Command::SetTimer(range) => {
            let duration = if range.start < range.end {
                use rand::Rng;
                rand::thread_rng().gen_range(range.start, range.end)
            } else {
                range.start
            };
            *next_interrupt = Instant::now() + duration;
        }
        Command::CancelTimer => {
            *next_interrupt = practically_never();
        }
    }
}

#[cfg(test)]
mod test {
    use crate::actor::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

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
}
