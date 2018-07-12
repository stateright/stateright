#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate stateright;

mod state_machines;

use state_machines::write_once_register;
use stateright::actor;
use std::io::Result;

fn main() -> Result<()> {
    let port = 3000;

    println!("  This is a server written using the stateright actor library.");
    println!("  The server implements a single write-once register.");
    println!("  You can interact with the server using netcat. Example:");
    println!("$ nc -u 0 {}", port);
    println!("{}", serde_json::to_string(&write_once_register::Msg::Put { value: 'X' }).unwrap());
    println!("{}", serde_json::to_string(&write_once_register::Msg::Get).unwrap());
    println!();

    actor::spawn(write_once_register::Cfg::Server, format!("127.0.0.1:{}", port).parse().unwrap())
}
