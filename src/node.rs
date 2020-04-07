use crate::helper_fns::*;
use crate::quinn_ext;
use async_std::net::{TcpListener, TcpStream};
use chashmap::CHashMap;
use futures::{future::FutureExt, pin_mut, prelude::*, select};
use p2p_node_stats::Stats;
use serde::{Deserialize, Serialize};
use std::{io, net::SocketAddr, time::Duration};

// Buffer size in bytes.
const BUFFER_SIZE: usize = 2024;
const PING_SIZE: usize = 32;

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
    tx_bytes: usize,
    tx_interval_sec: usize,
    node_ttl: f64,
    quic: bool,
}

impl Node {
    pub fn new(
        listen_address: SocketAddr,
        tx_bytes: usize,
        tx_interval_sec: usize,
        node_ttl: f64,
        stats_window_size: usize,
        use_quic: bool,
    ) -> Node {
        Node {
            peers: CHashMap::new(),
            listen_address,
            sent_pings: CHashMap::new(),
            stats: Stats::new(stats_window_size, listen_address.to_string()),
            tx_bytes,
            tx_interval_sec,
            node_ttl,
            quic: use_quic,
        }
    }

    pub async fn start(&self) -> io::Result<()> {
        let listen_future = self.listen_and_reconnect().fuse();

        pin_mut!(listen_future);

        let mut tx_interval =
            async_std::stream::interval(Duration::from_secs(self.tx_interval_sec as u64));
        let mut ping_interval = async_std::stream::interval(Duration::from_secs(15));
        let mut exit_interval = async_std::stream::interval(Duration::from_secs_f64(self.node_ttl));

        loop {
            let ping_future = ping_interval.next().fuse();
            let tx_future = tx_interval.next().fuse();
            let exit_future = exit_interval.next().fuse();

            pin_mut!(ping_future, tx_future, exit_future);

            select! {
                    listen = listen_future => unreachable!(),
                    ping = ping_future => self.ping_all().await?,
                    tx = tx_future => self.broadcast_tx().await?,
                    exit = exit_future => {
                        self.broadcast(&Message::RemovePeer(Peer {
                            listen_port: self.listen_address.port(),
                        }))
                        .await?;
                        info!("Shutting down.");
                        break;
                    },
            };
        }
        self.stats.save_to_file("stats.txt")?;
        Ok(())
    }

    async fn broadcast_tx(&self) -> io::Result<()> {
        let tx = Tx {
            payload: gen_random_bytes(self.tx_bytes),
            peer: Peer {
                listen_port: self.listen_address.port(),
            },
            sent_time: current_time(),
        };
        info!("Sending tx.");
        self.broadcast(&Message::Tx(tx)).await
    }

    async fn listen_and_reconnect(&self) {
        loop {
            match self.listen().await {
                Ok(_) => unreachable!(),
                Err(_) => (),
            }
        }
    }
    async fn listen(&self) -> io::Result<()> {
        if self.quic {
            self.listen_quic().await
        } else {
            self.listen_tcp().await
        }
    }

    async fn listen_tcp(&self) -> io::Result<()> {
        let listener = TcpListener::bind(self.listen_address).await?;
        let mut incoming = listener.incoming();

        println!("TCP: Listening on {}", self.listen_address);
        while let Some(stream) = incoming.next().await {
            let mut stream = stream?;
            let mut buffer = [0u8; BUFFER_SIZE];
            let _read_size = stream.read(&mut buffer).await?;
            self.handle_message(&buffer, stream.peer_addr()?).await?;
            stream.flush().await?;
        }
        Ok(())
    }

    async fn listen_quic(&self) -> io::Result<()> {
        let (mut incoming, _server_cert) = quinn_ext::make_server_endpoint(self.listen_address)
            .expect("Failed to initialize QUIC listener.");

        println!("QUIC: Listening on {}", self.listen_address);
        while let Some(conn_stream) = incoming.next().await {
            let quinn::NewConnection {
                connection,
                mut bi_streams,
                ..
            } = conn_stream.await?;
            async {
                // Each stream initiated by the client constitutes a new request.
                while let Some(stream) = bi_streams.next().await {
                    let (_send, mut recv): (quinn::SendStream, quinn::RecvStream) = match stream {
                        Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                            return Ok(());
                        }
                        Err(e) => {
                            return Err(e);
                        }
                        Ok(s) => s,
                    };
                    let mut buffer = [0u8; BUFFER_SIZE];
                    let _read_size = recv
                        .read(&mut buffer)
                        .await
                        .expect("Failed reading the incoming message.");
                    self.handle_message(&buffer, connection.remote_address())
                        .await
                        .expect("Failed handling the incoming message.");
                }
                Ok(())
            }
            .await?;
        }
        Ok(())
    }

    pub async fn start_and_connect(&self, peer_address: &str) -> io::Result<()> {
        let peer_address = peer_address.parse().expect("Failed to parse peer ip.");
        self.peers.insert(peer_address, peer_address);
        let message = Message::NewPeer(Peer {
            listen_port: self.listen_address.port(),
        });
        self.send(&message, peer_address).await?;
        self.start().await?;
        Ok(())
    }

    async fn send(&self, message: &Message, address: SocketAddr) -> io::Result<()> {
        if self.quic {
            Self::send_quic(message, address).await
        } else {
            Self::send_tcp(message, address).await
        }
    }

    async fn send_tcp(message: &Message, address: SocketAddr) -> io::Result<()> {
        let mut stream = TcpStream::connect(address).await?;
        let serialized = bincode::serialize(message).expect("Failed to serialize a message.");
        stream.write_all(serialized.as_ref()).await?;
        stream.flush().await?;
        //info!("Sent {:?} to {}", message, stream.peer_addr()?);
        Ok(())
    }

    async fn send_quic(message: &Message, address: SocketAddr) -> io::Result<()> {
        let endpoint =
            quinn_ext::make_client_endpoint_untrusted().expect("Failed to make client endpoint.");
        let new_conn = endpoint
            .connect(&address, "localhost")
            .expect("QUIC: Failed to connect.")
            .await
            .expect("QUIC: Failed to connect.");
        let quinn::NewConnection {
            connection: conn, ..
        } = { new_conn };
        let (mut send, _recv) = conn
            .open_bi()
            .await
            .expect("QUIC: Failed to open write stream.");
        let serialized = bincode::serialize(message).expect("Failed to serialize a message.");
        send.write_all(serialized.as_slice())
            .await
            .expect("QUIC: Failed to send message");
        send.finish().await.expect("QUIC: Failed to finish sending");
        conn.close(0u32.into(), b"done");
        //info!("Sent {:?} to {}", message, stream.peer_addr()?);
        Ok(())
    }

    async fn broadcast(&self, message: &Message) -> io::Result<()> {
        for (peer, _) in self.peers.clone() {
            self.send(message, peer).await?;
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
        self.send(&Message::Ping(ping), address).await
    }

    async fn ping_all(&self) -> io::Result<()> {
        for (peer, _) in self.peers.clone() {
            self.ping(peer).await?;
        }
        Ok(())
    }

    async fn handle_message(&self, bytes: &[u8], remote_address: SocketAddr) -> io::Result<()> {
        let message: Message =
            bincode::deserialize(bytes).expect("Failed to deserialize a message.");
        //info!("Got {:?}", message);
        match message {
            Message::Ping(ping) => {
                self.send(
                    &Message::Pong(ping.clone()),
                    addr_with_port(remote_address, ping.from_peer.listen_port),
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
                        addr_with_port(remote_address, ping.to_peer.listen_port).to_string();
                    let rtt = current_time() - sent_time.to_owned();
                    self.stats.add_ping(peer_address.clone(), rtt);
                    info!("Ping to {} returned in {:?}.", peer_address, rtt);
                }
            }
            Message::Tx(tx) => {
                let peer_address = addr_with_port(remote_address, tx.peer.listen_port).to_string();
                let time = current_time() - tx.sent_time;
                info!("Received tx from {} in {:?}", peer_address, time);
                self.stats
                    .add_transmission(peer_address, time, tx.payload.len() as u32);
            }
            Message::NewPeer(peer) => {
                let mut peer_address = remote_address;
                peer_address.set_port(peer.listen_port);
                info!(
                    "Received request to add new peer {} to swarm.",
                    peer_address
                );

                //tell new node about other peers
                info!("Telling new node about other peers.");
                for (peer, _) in self.peers.clone() {
                    let message = Message::AddPeer(peer);
                    self.send(&message, peer_address).await?
                }

                //tell other peers about the new node
                info!("Telling other peers about new node.");
                let message = Message::AddPeer(peer_address);
                self.broadcast(&message).await?;

                //remember new node
                self.peers.insert(peer_address, peer_address);
                info!("Added peer {}", peer_address);
            }
            Message::AddPeer(address) => {
                self.peers.insert(address, address);
                info!("Added peer {}", address);
            }
            Message::RemovePeer(peer) => {
                let peer_address = addr_with_port(remote_address, peer.listen_port);
                self.peers.remove(&peer_address);
                info!("Removed peer {}", peer_address);
            }
        }
        Ok(())
    }
}
