use async_std::{
    net::{TcpListener, TcpStream},
    prelude::*,
};
use futures::executor::block_on;
use serde::{Deserialize, Serialize};
use std::{env, io, net::SocketAddr, time::Duration};
extern crate clap;
use clap::{value_t, App, Arg, SubCommand};

const LISTEN_ON_IP: &str = "127.0.0.1";
const LISTEN_ON_PORT: u16 = 7878;

// Buffer size in bytes.
const BUFFER_SIZE: usize = 512;
const PING_SIZE: usize = 32;
const TX_SIZE: usize = 200;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum Message {
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Tx(Vec<u8>),
    AddPeer(SocketAddr),
    NewPeer(Peer),
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Peer {
    listen_port: u16,
}

use chashmap::CHashMap;
use rand::distributions::Standard;
use rand::{thread_rng, Rng};
use std::net::IpAddr;

pub fn gen_random_bytes(n_bytes: usize) -> Vec<u8> {
    let rng = thread_rng();
    rng.sample_iter(Standard).take(n_bytes).collect()
}

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
    let mut node = Node::new(listen_address);
    match matches.value_of("connect") {
        Some(address) => node.start_and_connect(address).await?,
        None => node.start().await?,
    }
    Ok(())
}

struct Node {
    //It's a hash set, done with hashmap
    peers: CHashMap<SocketAddr, SocketAddr>,
    listen_address: SocketAddr,
}

impl Node {
    fn new(listen_address: SocketAddr) -> Node {
        Node {
            peers: CHashMap::new(),
            listen_address,
        }
    }

    async fn start(&mut self) -> io::Result<()> {
        let listener = TcpListener::bind(self.listen_address).await?;
        let mut incoming = listener.incoming();
        println!("Listening on {}", self.listen_address);

        while let Some(stream) = incoming.next().await {
            let mut stream = stream?;
            let mut buffer = [0u8; BUFFER_SIZE];
            let _read_size = stream.read(&mut buffer).await?;
            self.handle_message(&buffer, &mut stream).await?;
            stream.flush().await?;
        }
        Ok(())
    }

    async fn start_and_connect(&mut self, peer_address: &str) -> io::Result<()> {
        let peer_address = peer_address.parse().expect("Failed to parse peer ip.");
        self.peers.insert(peer_address, peer_address);
        let message = Message::NewPeer(Peer {
            listen_port: self.listen_address.port(),
        });
        Node::send(&message, peer_address).await?;
        self.start().await?;
        Ok(())
    }

    async fn send(message: &Message, address: SocketAddr) -> io::Result<()> {
        let mut stream = TcpStream::connect(address).await?;
        let serialized = bincode::serialize(message).expect("Failed to serialize a message.");
        stream.write_all(serialized.as_ref()).await?;
        stream.flush().await?;
        println!("Sent {:?} to {}", message, stream.peer_addr()?);
        Ok(())
    }

    async fn broadcast(&self, message: &Message) -> io::Result<()> {
        for (peer, _) in self.peers.clone() {
            Node::send(message, peer).await?
        }
        Ok(())
    }

    async fn handle_message(&mut self, bytes: &[u8], stream: &mut TcpStream) -> io::Result<()> {
        let message: Message =
            bincode::deserialize(bytes).expect("Failed to deserialize a message.");
        println!("Got {:?}", message);
        match message {
            Message::Ping(bytes) => (),
            Message::Pong(bytes) => unimplemented!(),
            Message::Tx(bytes) => unimplemented!(),
            Message::NewPeer(peer) => {
                let mut peer_address = stream.peer_addr()?;
                peer_address.set_port(peer.listen_port);
                println!(
                    "Received request to add new peer {} to swarm.",
                    peer_address
                );

                //tell new node about other peers
                println!("Telling new node about other peers.");
                for (peer, _) in self.peers.clone() {
                    let message = Message::AddPeer(peer);
                    Node::send(&message, peer_address).await?
                }

                //tell other peers about the new node
                println!("Telling other peers about new node.");
                let message = Message::AddPeer(peer_address);
                self.broadcast(&message).await?;

                //remember new node
                self.peers.insert(peer_address, peer_address);
                println!("Added peer {}", peer_address);
            }
            Message::AddPeer(address) => {
                self.peers.insert(address, address);
                println!("Added peer {}", address);
            }
        }
        Ok(())
    }
}

fn main() -> io::Result<()> {
    block_on(async_main())
}
