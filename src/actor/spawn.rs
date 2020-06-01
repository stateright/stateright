//! A simple runtime for executing an actor mapping messages to JSON over UDP.

use crate::actor::*;
use serde::de::DeserializeOwned;
use serde::ser::Serialize;
use std::fmt::Debug;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::thread;
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

/// Runs an actor by mapping messages to JSON over UDP.
pub fn spawn<A>(actor: A, id: impl Into<Id>) -> thread::JoinHandle<()>
where
    A: 'static + Send + Actor,
    A::Msg: Debug + DeserializeOwned + Serialize,
    A::State: Debug,
{
    let id = id.into();
    let addr = SocketAddrV4::from(id);

    // note that panics are returned as `Err` when `join`ing
    thread::spawn(move || {
        let socket = UdpSocket::bind(addr).unwrap(); // panic if unable to bind
        let mut in_buf = [0; 65_535];
        let mut next_interrupt = practically_never();

        let mut last_state = {
            let out = actor.on_start_out(id);
            log::info!("Actor started. id={}, state={:?}, commands={:?}", addr, out.state, out.commands);
            for c in out.commands {
                on_command::<A>(addr, c, &socket, &mut next_interrupt);
            }
            out.state.expect("actor not initialized")
        };
        loop {
            // Apply an interrupt if present, otherwise wait for a message.
            let out =
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
                            match A::deserialize(&in_buf[..count]) {
                                Ok(msg) => {
                                    if let SocketAddr::V4(src_addr) = src_addr {
                                        log::info!("Received message. id={}, src={}, msg={}",
                                                    addr, src_addr, format!("{:?}", msg));
                                        actor.on_msg_out(id, &last_state, Id::from(src_addr), msg)
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
                    actor.on_timeout_out(id, &last_state)
                };

            // Handle commands and update state.
            if !out.commands.is_empty() || out.state.is_some() {
                log::debug!("Acted. id={}, last_state={:?}, next_state={:?}, commands={:?}",
                            addr, last_state, out.state, out.commands);
            }
            for c in out.commands { on_command::<A>(addr, c, &socket, &mut next_interrupt); }
            if let Some(next_state) = out.state { last_state = next_state; }
        }
    })
}

/// The effect to perform in response to spawned actor outputs.
fn on_command<A: Actor>(addr: SocketAddrV4, command: Command<A::Msg>, socket: &UdpSocket, next_interrupt: &mut Instant)
where A::Msg: Debug + Serialize
{
    match command {
        Command::Send(dst, msg) => {
            let dst_addr = SocketAddrV4::from(dst);
            match A::serialize(&msg) {
                Err(e) => {
                    log::warn!("Unable to serialize. Ignoring. src={}, dst={}, msg={:?}, err={}",
                             addr, dst_addr, msg, e);
                },
                Ok(out_buf) => {
                    if let Err(e) = socket.send_to(&out_buf, dst_addr) {
                        log::warn!("Unable to send. Ignoring. src={}, dst={}, msg={:?}, err={}",
                                 addr, dst_addr, msg, e);
                    }
                },
            }
        },
        Command::SetTimer(range) => {
            let duration =
                if range.start < range.end {
                    use rand::Rng;
                    rand::thread_rng().gen_range(range.start, range.end)
                } else {
                    range.start
                };
            *next_interrupt = Instant::now() + duration;
        },
        Command::CancelTimer => {
            *next_interrupt = practically_never();
        },
    }
}

#[cfg(test)]
mod test {
    use crate::actor::*;
    use std::net::{SocketAddrV4, Ipv4Addr};

    #[test]
    fn can_encode_id() {
        let addr = SocketAddrV4::new(Ipv4Addr::new(1,2,3,4), 5);
        assert_eq!(
            Id::from(addr).0.to_be_bytes(),
            [0, 0, 1, 2, 3, 4, 0, 5]);
    }

    #[test]
    fn can_decode_id() {
        let addr = SocketAddrV4::new(Ipv4Addr::new(1,2,3,4), 5);
        assert_eq!(
            SocketAddrV4::from(Id::from(addr)),
            addr);
    }
}
