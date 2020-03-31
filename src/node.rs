use crate::helper_fns::*;
use async_std::net::{TcpListener, TcpStream};
use chashmap::CHashMap;
use futures::future::Ready;
use futures::{
    future::FutureExt,
    pin_mut,
    prelude::*,
    select,
    task::{self, Poll},
};
use serde::{Deserialize, Serialize};
use std::{io, net::SocketAddr, time::Duration};

// Buffer size in bytes.
const BUFFER_SIZE: usize = 512;
const PING_SIZE: usize = 32;
const TX_SIZE: usize = 200;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum Message {
    Ping(Ping),
    Pong(Ping),
    Tx(Vec<u8>),
    AddPeer(SocketAddr),
    NewPeer(Peer),
    RemovePeer(Peer),
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Hash)]
struct Ping {
    payload: Vec<u8>,
    peer: Peer,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Hash)]
struct Peer {
    listen_port: u16,
}

pub struct Node {
    //It's a hash set, done with hashmap
    peers: CHashMap<SocketAddr, SocketAddr>,
    listen_address: SocketAddr,
    sent_pings: CHashMap<Ping, Duration>,
}

impl Node {
    pub fn new(listen_address: SocketAddr) -> Node {
        Node {
            peers: CHashMap::new(),
            listen_address,
            sent_pings: CHashMap::new(),
        }
    }

    pub async fn start(&self) -> io::Result<()> {
        let listen_future = self.listen().fuse();
        let mut tx_interval = async_std::stream::interval(Duration::from_secs(10));
        let mut ping_interval = async_std::stream::interval(Duration::from_secs(15));
        let mut exit_interval = async_std::stream::interval(Duration::from_secs(100));

        pin_mut!(listen_future);

        loop {
            let ping_future = ping_interval.next().fuse();
            let tx_future = tx_interval.next().fuse();
            let exit_future = exit_interval.next().fuse();

            pin_mut!(ping_future, tx_future, exit_future);

            select! {
                    listen = listen_future => unreachable!(),
                    ping = ping_future => self.ping_all().await?,
                    tx = tx_future => self.broadcast(&Message::Tx(gen_random_bytes(64))).await?,
                    exit = exit_future => {
                        self.broadcast(&Message::RemovePeer(Peer {
                            listen_port: self.listen_address.port(),
                        }))
                        .await?;
                        println!("Shutting down.");
                        break;
                    },
            };
        }
        Ok(())
    }

    async fn listen(&self) -> io::Result<()> {
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

    pub async fn start_and_connect(&self, peer_address: &str) -> io::Result<()> {
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
            Node::send(message, peer).await?;
        }
        Ok(())
    }

    async fn ping(&self, address: SocketAddr) -> io::Result<()> {
        //TODO: add receiver address to ping payload?
        let ping = Ping {
            payload: gen_random_bytes(PING_SIZE),
            peer: Peer {
                listen_port: self.listen_address.port(),
            },
        };
        self.sent_pings.insert(ping.clone(), current_time());
        Node::send(&Message::Ping(ping), address).await
    }

    async fn ping_all(&self) -> io::Result<()> {
        for (peer, _) in self.peers.clone() {
            self.ping(peer).await?;
        }
        Ok(())
    }

    async fn handle_message(&self, bytes: &[u8], stream: &mut TcpStream) -> io::Result<()> {
        let message: Message =
            bincode::deserialize(bytes).expect("Failed to deserialize a message.");
        println!("Got {:?}", message);
        match message {
            Message::Ping(ping) => {
                Node::send(
                    &Message::Pong(ping.clone()),
                    addr_with_port(stream.peer_addr()?, ping.peer.listen_port),
                )
                .await?;
            }
            Message::Pong(ping) => {
                if self.sent_pings.contains_key(&ping) {
                    let sent_time = self
                        .sent_pings
                        .get(&ping)
                        .expect("Failed to get sent ping entry.");
                    println!(
                        "Ping to {:?} returned in {:?}.",
                        ping.peer,
                        current_time() - sent_time.to_owned()
                    )
                }
            }
            Message::Tx(bytes) => (),
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
            Message::RemovePeer(peer) => {
                let peer_address = addr_with_port(stream.peer_addr()?, peer.listen_port);
                self.peers.remove(&peer_address);
                println!("Removed peer {}", peer_address);
            }
        }
        Ok(())
    }
}
