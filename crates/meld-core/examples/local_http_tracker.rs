use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
struct Peer {
    id: Vec<u8>,
    address: SocketAddr,
}

fn main() -> Result<()> {
    let bind: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:45991".to_owned())
        .parse()
        .context("invalid bind address")?;
    if !bind.ip().is_loopback() {
        bail!("the experiment tracker only permits loopback bind addresses");
    }
    let listener = TcpListener::bind(bind).with_context(|| format!("bind tracker {bind}"))?;
    println!("local_tracker_started bind={}", listener.local_addr()?);
    let mut swarms: HashMap<Vec<u8>, Vec<Peer>> = HashMap::new();
    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                if let Err(error) = handle_announce(stream, &mut swarms) {
                    eprintln!("tracker_request_error={error:#}");
                }
            }
            Err(error) => eprintln!("tracker_accept_error={error}"),
        }
    }
    Ok(())
}

fn handle_announce(mut stream: TcpStream, swarms: &mut HashMap<Vec<u8>, Vec<Peer>>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let remote = stream.peer_addr()?;
    let request = read_headers(&mut stream)?;
    let request_line = request
        .lines()
        .next()
        .context("missing HTTP request line")?;
    let mut request_parts = request_line.split_whitespace();
    if request_parts.next() != Some("GET") {
        bail!("only GET is supported");
    }
    let target = request_parts
        .next()
        .context("missing HTTP request target")?;
    let (_, query) = target
        .split_once('?')
        .context("announce query is missing")?;
    let parameters = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| Ok((key, percent_decode(value)?)))
        .collect::<Result<HashMap<_, _>>>()?;
    let info_hash = parameters
        .get("info_hash")
        .context("info_hash is missing")?
        .clone();
    let peer_id = parameters
        .get("peer_id")
        .context("peer_id is missing")?
        .clone();
    if info_hash.len() != 20 || peer_id.len() != 20 {
        bail!("info_hash and peer_id must each contain 20 bytes");
    }
    let port: u16 = std::str::from_utf8(parameters.get("port").context("port is missing")?)
        .context("port is not UTF-8")?
        .parse()
        .context("port is invalid")?;
    let event = parameters
        .get("event")
        .map(Vec::as_slice)
        .unwrap_or_default();
    let address = SocketAddr::new(remote.ip(), port);
    let swarm = swarms.entry(info_hash.clone()).or_default();
    swarm.retain(|peer| peer.id != peer_id);
    if event != b"stopped" && port != 0 {
        swarm.push(Peer {
            id: peer_id.clone(),
            address,
        });
    }

    let visible: Vec<SocketAddr> = swarm
        .iter()
        .filter(|peer| peer.id != peer_id)
        .map(|peer| peer.address)
        .collect();
    let mut compact = Vec::new();
    for peer in &visible {
        if let IpAddr::V4(ip) = peer.ip() {
            compact.extend_from_slice(&ip.octets());
            compact.extend_from_slice(&peer.port().to_be_bytes());
        }
    }
    let mut body = b"d8:intervali60e5:peers".to_vec();
    body.extend_from_slice(compact.len().to_string().as_bytes());
    body.push(b':');
    body.extend_from_slice(&compact);
    body.push(b'e');
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    stream.flush()?;
    println!(
        "tracker_announce info_hash={} peer={} event={} peers_returned={}",
        hex::encode(info_hash),
        address,
        String::from_utf8_lossy(event),
        compact.len() / 6
    );
    Ok(())
}

fn read_headers(stream: &mut TcpStream) -> Result<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 2048];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > 16 * 1024 {
            bail!("HTTP tracker request headers are too large");
        }
    }
    String::from_utf8(bytes).context("HTTP tracker request is not UTF-8")
}

fn percent_decode(input: &str) -> Result<Vec<u8>> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let high = *bytes.get(index + 1).context("truncated percent escape")?;
                let low = *bytes.get(index + 2).context("truncated percent escape")?;
                decoded.push((hex_digit(high)? << 4) | hex_digit(low)?);
                index += 3;
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    Ok(decoded)
}

fn hex_digit(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!("invalid percent escape"),
    }
}
