use futures::executor::block_on;
use std::{io, net::SocketAddr};
extern crate clap;
use crate::node::Node;
use clap::{value_t, App, Arg};

pub mod helper_fns;
pub mod node;

const LISTEN_ON_IP: &str = "127.0.0.1";
const LISTEN_ON_PORT: u16 = 7878;

async fn async_main() -> io::Result<()> {
    let matches = App::new("TCP p2p example node")
        .version("0.1")
        .author("Egor Ivkov e.o.ivkov@gmail.com")
        .about("-")
        .arg(
            Arg::with_name("connect")
                .short("c")
                .long("connect")
                .value_name("SocketAddr")
                .help("Establishes connection with a swarm through the specified node.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("port")
                .short("p")
                .long("port")
                .value_name("u16")
                .help("Port to listen on.")
                .takes_value(true),
        )
        .get_matches();
    let port = value_t!(matches, "port", u16).unwrap_or(LISTEN_ON_PORT);
    let listen_address = SocketAddr::new(LISTEN_ON_IP.parse().expect("Failed to parse ip."), port);
    let node = Node::new(listen_address);
    match matches.value_of("connect") {
        Some(address) => node.start_and_connect(address).await?,
        None => node.start().await?,
    }
    Ok(())
}

fn main() -> io::Result<()> {
    block_on(async_main())
}
