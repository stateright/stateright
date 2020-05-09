//! A simple runtime for executing an actor state machine mapping messages to JSON over UDP.

use crate::actor::*;
use serde::de::DeserializeOwned;
use serde::ser::Serialize;
use std::fmt::Debug;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::thread;

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

        let mut last_state = {
            let out = actor.init_out(id);
            println!(
                "Actor started. id={}, state={:?}, commands={:?}",
                addr, out.state, out.commands
            );
            for c in out.commands {
                on_command::<A>(addr, c, &socket);
            }
            out.state.expect("actor not initialized")
        };
        loop {
            let (count, src_addr) = socket.recv_from(&mut in_buf).unwrap(); // panic if unable to read
            match A::deserialize(&in_buf[..count]) {
                Ok(msg) => {
                    println!(
                        "Received message. dst={}, src={}, msg={:?}",
                        addr, src_addr, msg
                    );

                    if let SocketAddr::V4(src_addr) = src_addr {
                        let out = actor.next_out(
                            id,
                            &last_state,
                            Event::Receive(Id::from(src_addr), msg),
                        );
                        println!(
                            "Actor advanced. id={}, state={:?}, commands={:?}",
                            addr, out.state, out.commands
                        );
                        for c in out.commands {
                            on_command::<A>(src_addr, c, &socket);
                        }
                        if let Some(next_state) = out.state {
                            last_state = next_state;
                        }
                    } else {
                        println!(
                            "Source is not IPv4. Ignoring. id={}, src={}, msg={:?}",
                            addr, src_addr, msg
                        );
                    }
                }
                Err(e) => {
                    println!(
                        "Unable to parse message. Ignoring. id={}, src={}, buf={:?}, err={}",
                        addr,
                        src_addr,
                        &in_buf[..count],
                        e
                    );
                }
            }
        }
    })
}

/// The effect to perform in response to spawned actor outputs.
fn on_command<A: Actor>(addr: SocketAddrV4, command: Command<A::Msg>, socket: &UdpSocket)
where
    A::Msg: Debug + Serialize,
{
    let Command::Send(dst, msg) = command;
    let dst_addr = SocketAddrV4::from(dst);
    match A::serialize(&msg) {
        Err(e) => {
            println!(
                "Unable to serialize. Ignoring. src={}, dst={}, msg={:?}, err={}",
                addr, dst_addr, msg, e
            );
        }
        Ok(out_buf) => {
            if let Err(e) = socket.send_to(&out_buf, dst_addr) {
                println!(
                    "Unable to send. Ignoring. src={}, dst={}, msg={:?}, err={}",
                    addr, dst_addr, msg, e
                );
            }
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
