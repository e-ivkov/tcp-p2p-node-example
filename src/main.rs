use async_std::{
    net::{TcpListener, TcpStream},
    prelude::*,
};
use futures::executor::block_on;
use serde::{Deserialize, Serialize};
use std::env;
use std::io;
use std::time::Duration;

const LISTEN_ON: &str = "127.0.0.1:7878";

// Buffer size in bytes.
const BUFFER_SIZE: usize = 512;
const PING_SIZE: usize = 32;
const TX_SIZE: usize = 200;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum Message {
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Tx(Vec<u8>),
}

use rand::distributions::Standard;
use rand::{thread_rng, Rng};

pub fn gen_random_bytes(n_bytes: usize) -> Vec<u8> {
    let rng = thread_rng();
    rng.sample_iter(Standard).take(n_bytes).collect()
}

async fn async_main() -> io::Result<()> {
    let mut args: Vec<String> = env::args().collect();
    match args.pop().as_ref().map(String::as_str) {
        Some("--send") => send().await?,
        _ => listen().await?,
    }
    Ok(())
}

async fn listen() -> io::Result<()> {
    let listener = TcpListener::bind(LISTEN_ON).await?;
    let mut incoming = listener.incoming();
    //let (stream, addr) = listener.accept().await?;
    println!("Listening on {}", LISTEN_ON);

    while let Some(stream) = incoming.next().await {
        let mut stream = stream?;
        let mut buffer = [0u8; BUFFER_SIZE];
        let _read_size = stream.read(&mut buffer).await?;
        handle_message(&buffer);
        stream.flush().await?;
    }
    Ok(())
}

async fn send() -> io::Result<()> {
    let mut stream = TcpStream::connect(LISTEN_ON).await?;
    let message = Message::Ping(gen_random_bytes(PING_SIZE));
    let serialized = bincode::serialize(&message).expect("Failed to serialize a message.");
    stream.write_all(serialized.as_ref()).await?;
    stream.flush().await?;
    println!("Sent {:?}", message);
    Ok(())
}

fn handle_message(bytes: &[u8]) {
    let message: Message = bincode::deserialize(bytes).expect("Failed to deserialize message.");
    println!("Got {:?}", message);
}

fn main() -> io::Result<()> {
    block_on(async_main())
}
