use rand::distributions::Standard;
use rand::{thread_rng, Rng};
use std::{
    net::SocketAddr,
    time::{Duration, SystemTime},
};

pub fn gen_random_bytes(n_bytes: usize) -> Vec<u8> {
    let rng = thread_rng();
    rng.sample_iter(Standard).take(n_bytes).collect()
}

pub fn current_time() -> Duration {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("Failed to get duration since UNIX_EPOCH.")
}

pub fn addr_with_port(address: SocketAddr, port: u16) -> SocketAddr {
    let mut address = address;
    address.set_port(port);
    address
}
