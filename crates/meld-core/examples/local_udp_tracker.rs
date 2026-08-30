use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, UdpSocket};

use anyhow::{Context, Result, bail};

const PROTOCOL_ID: u64 = 0x0417_2710_1980;
const CONNECTION_ID: u64 = 0x5348_4152_444d_454c;

#[derive(Debug, Clone)]
struct Peer {
    id: [u8; 20],
    address: SocketAddr,
}

fn main() -> Result<()> {
    let bind: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:45993".to_owned())
        .parse()
        .context("invalid bind address")?;
    if !bind.ip().is_loopback() {
        bail!("the experiment tracker only permits loopback bind addresses");
    }
    let socket = UdpSocket::bind(bind).with_context(|| format!("bind UDP tracker {bind}"))?;
    println!("local_udp_tracker_started bind={}", socket.local_addr()?);
    let mut swarms: HashMap<[u8; 20], Vec<Peer>> = HashMap::new();
    let mut buffer = [0_u8; 65_507];
    loop {
        let (length, remote) = socket.recv_from(&mut buffer)?;
        if let Err(error) = handle_packet(&socket, remote, &buffer[..length], &mut swarms) {
            eprintln!("udp_tracker_request_error remote={remote} error={error:#}");
        }
    }
}

fn handle_packet(
    socket: &UdpSocket,
    remote: SocketAddr,
    packet: &[u8],
    swarms: &mut HashMap<[u8; 20], Vec<Peer>>,
) -> Result<()> {
    if packet.len() < 16 {
        bail!("packet is shorter than 16 bytes");
    }
    let action = u32::from_be_bytes(packet[8..12].try_into()?);
    let transaction = u32::from_be_bytes(packet[12..16].try_into()?);
    match action {
        0 => {
            if u64::from_be_bytes(packet[..8].try_into()?) != PROTOCOL_ID {
                bail!("wrong UDP tracker protocol ID");
            }
            let mut response = Vec::with_capacity(16);
            response.extend_from_slice(&0_u32.to_be_bytes());
            response.extend_from_slice(&transaction.to_be_bytes());
            response.extend_from_slice(&CONNECTION_ID.to_be_bytes());
            socket.send_to(&response, remote)?;
        }
        1 => handle_announce(socket, remote, transaction, packet, swarms)?,
        _ => send_error(socket, remote, transaction, "unsupported action")?,
    }
    Ok(())
}

fn handle_announce(
    socket: &UdpSocket,
    remote: SocketAddr,
    transaction: u32,
    packet: &[u8],
    swarms: &mut HashMap<[u8; 20], Vec<Peer>>,
) -> Result<()> {
    if packet.len() < 98 {
        bail!("announce packet is shorter than 98 bytes");
    }
    if u64::from_be_bytes(packet[..8].try_into()?) != CONNECTION_ID {
        send_error(socket, remote, transaction, "invalid connection ID")?;
        return Ok(());
    }
    let info_hash: [u8; 20] = packet[16..36].try_into()?;
    let peer_id: [u8; 20] = packet[36..56].try_into()?;
    let event = u32::from_be_bytes(packet[80..84].try_into()?);
    let port = u16::from_be_bytes(packet[96..98].try_into()?);
    let peer_address = SocketAddr::new(remote.ip(), port);
    let swarm = swarms.entry(info_hash).or_default();
    swarm.retain(|peer| peer.id != peer_id);
    if event != 3 && port != 0 {
        swarm.push(Peer {
            id: peer_id,
            address: peer_address,
        });
    }

    let mut compact = Vec::new();
    for peer in swarm.iter().filter(|peer| peer.id != peer_id) {
        if let IpAddr::V4(ip) = peer.address.ip() {
            compact.extend_from_slice(&ip.octets());
            compact.extend_from_slice(&peer.address.port().to_be_bytes());
        }
    }
    let mut response = Vec::with_capacity(20 + compact.len());
    response.extend_from_slice(&1_u32.to_be_bytes());
    response.extend_from_slice(&transaction.to_be_bytes());
    response.extend_from_slice(&60_u32.to_be_bytes());
    response.extend_from_slice(&0_u32.to_be_bytes());
    response.extend_from_slice(&(swarm.len() as u32).to_be_bytes());
    response.extend_from_slice(&compact);
    socket.send_to(&response, remote)?;
    println!(
        "udp_tracker_announce info_hash={} peer={} event={} peers_returned={}",
        hex::encode(info_hash),
        peer_address,
        event,
        compact.len() / 6
    );
    Ok(())
}

fn send_error(
    socket: &UdpSocket,
    remote: SocketAddr,
    transaction: u32,
    message: &str,
) -> Result<()> {
    let mut response = Vec::with_capacity(8 + message.len());
    response.extend_from_slice(&3_u32.to_be_bytes());
    response.extend_from_slice(&transaction.to_be_bytes());
    response.extend_from_slice(message.as_bytes());
    socket.send_to(&response, remote)?;
    Ok(())
}
