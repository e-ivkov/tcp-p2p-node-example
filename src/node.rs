use crate::helper_fns::*;
use async_std::net::{TcpListener, TcpStream};
use chashmap::CHashMap;
use futures::{future::FutureExt, pin_mut, prelude::*, select};
use p2p_node_stats::Stats;
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
    Tx(Tx),
    AddPeer(SocketAddr),
    NewPeer(Peer),
    RemovePeer(Peer),
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
struct Tx {
    payload: Vec<u8>,
    peer: Peer,
    sent_time: Duration,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Hash)]
struct Ping {
    payload: Vec<u8>,
    to_peer: Peer,
    from_peer: Peer,
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
    stats: Stats,
}

impl Node {
    pub fn new(listen_address: SocketAddr) -> Node {
        Node {
            peers: CHashMap::new(),
            listen_address,
            sent_pings: CHashMap::new(),
            stats: Stats::new(100, listen_address.to_string()),
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
                    tx = tx_future => self.broadcast(&Message::Tx(self.gen_tx())).await?,
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
        self.stats.save_to_file("stats.txt")?;
        Ok(())
    }

    fn gen_tx(&self) -> Tx {
        Tx {
            payload: gen_random_bytes(TX_SIZE),
            peer: Peer {
                listen_port: self.listen_address.port(),
            },
            sent_time: current_time(),
        }
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
        let ping = Ping {
            payload: gen_random_bytes(PING_SIZE),
            to_peer: Peer {
                listen_port: address.port(),
            },
            from_peer: Peer {
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
                    addr_with_port(stream.peer_addr()?, ping.from_peer.listen_port),
                )
                .await?;
            }
            Message::Pong(ping) => {
                if self.sent_pings.contains_key(&ping) {
                    let sent_time = self
                        .sent_pings
                        .get(&ping)
                        .expect("Failed to get sent ping entry.");
                    let peer_address =
                        addr_with_port(stream.peer_addr()?, ping.to_peer.listen_port).to_string();
                    let rtt = current_time() - sent_time.to_owned();
                    self.stats.add_ping(peer_address.clone(), rtt);
                    println!("Ping to {} returned in {:?}.", peer_address, rtt);
                }
            }
            Message::Tx(tx) => {
                let peer_address =
                    addr_with_port(stream.peer_addr()?, tx.peer.listen_port).to_string();
                let time = current_time() - tx.sent_time;
                self.stats
                    .add_transmission(peer_address, time, tx.payload.len() as u32);
            }
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