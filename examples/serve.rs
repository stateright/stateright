extern crate serde_json;
extern crate stateright;

mod state_machines;

use state_machines::write_once_register::*;
use stateright::actor;
use stateright::actor::register::*;

fn main() {
    let port = 3000;

    println!("  This is a server written using the stateright actor library.");
    println!("  The server implements a single write-once register.");
    println!("  You can interact with the server using netcat. Example:");
    println!("$ nc -u 0 {}", port);
    println!("{}", serde_json::to_string(&RegisterMsg::Put::<Value, ()> { value: 'X' }).unwrap());
    println!("{}", serde_json::to_string(&RegisterMsg::Get::<Value, ()>).unwrap());
    println!();

    actor::spawn(RegisterCfg::Server(ServerCfg), ("127.0.0.1".parse().unwrap(), port)).join().unwrap();
}
