//! A simple runtime for executing an actor state machine mapping messages to JSON over UDP.

use crate::actor::*;
use std::net::{IpAddr, UdpSocket};
use std::thread;

/// An ID type for an actor identified by an IP address and port.
pub type SpawnId = (IpAddr, u16);

/// Runs an actor by mapping messages to JSON over UDP.
pub fn spawn<A>(actor: A, id: SpawnId) -> thread::JoinHandle<()>
where
    A: 'static + Send + Actor<SpawnId>,
    A::Msg: Debug + DeserializeOwned + Serialize,
    A::State: Debug,
{
    // note that panics are returned as `Err` when `join`ing
    thread::spawn(move || {
        let socket = UdpSocket::bind(id).unwrap(); // panic if unable to bind
        let mut in_buf = [0; 65_535];

        let mut result = actor.start();
        println!("Actor started. id={}, result={:#?}", fmt(&id), result);
        for o in &result.outputs.0 { on_output(&actor, o, &id, &socket); }

        loop {
            let (count, src_addr) = socket.recv_from(&mut in_buf).unwrap(); // panic if unable to read
            match actor.deserialize(&in_buf[..count]) {
                Ok(msg) => {
                    println!("Received message. id={}, src={}, msg={:?}", fmt(&id), src_addr, msg);

                    let input = ActorInput::Deliver { src: (src_addr.ip(), src_addr.port()), msg };
                    if let Some(new_result) = actor.advance(&result.state, &input) {
                        println!("Actor advanced. id={}, result={:#?}", fmt(&id), new_result);
                        result = new_result;
                        for o in &result.outputs.0 { on_output(&actor, o, &id, &socket); }
                    }
                },
                Err(e) => {
                    println!("Unable to parse message. Ignoring. id={}, src={}, buf={:?}, err={}",
                            fmt(&id), src_addr, &in_buf[..count], e);
                }
            }
        }
    })
}

/// The effect to perform in response to spawned actor outputs.
fn on_output<A: Actor<SpawnId>>(actor: &A, output: &ActorOutput<SpawnId, A::Msg>, id: &SpawnId, socket: &UdpSocket)
where A::Msg: Debug + Serialize
{
    let ActorOutput::Send { dst, msg } = output;
    match actor.serialize(msg) {
        Err(e) => {
            println!("Unable to serialize. Ignoring. id={}, dst={}, msg={:?}, err={}",
                     fmt(id), fmt(dst), msg, e);
        },
        Ok(out_buf) => {
            if let Err(e) = socket.send_to(&out_buf, &dst) {
                println!("Unable to send. Ignoring. id={}, dst={}, msg={:?}, err={}",
                         fmt(id), fmt(dst), msg, e);
            }
        },
    }
}

/// Convenience function for formatting the ID of a spawned actor.
fn fmt(id: &SpawnId) -> String { format!("{}:{}", id.0, id.1) }
