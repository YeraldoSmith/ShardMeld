use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;

use meld_core::{
    ChunkProfile, IndexDb, TorrentV1, create_descriptor, serve_v1_file, serve_v1_file_listener,
    serve_v1_index_listener,
};
use sha1::{Digest, Sha1};
use tempfile::tempdir;

fn deterministic_bytes(length: usize) -> Vec<u8> {
    let mut value = 0x510e_527f_ade6_82d1_u64;
    let mut bytes = Vec::with_capacity(length);
    for _ in 0..length {
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        bytes.push((value >> 24) as u8);
    }
    bytes
}

fn torrent_for(bytes: &[u8], piece_length: usize) -> TorrentV1 {
    TorrentV1 {
        name: "seed.bin".to_owned(),
        total_length: bytes.len() as u64,
        piece_length: piece_length as u64,
        piece_sha1: bytes
            .chunks(piece_length)
            .map(|piece| hex::encode(Sha1::digest(piece)))
            .collect(),
        info_hash_sha1: "11223344556677889900aabbccddeeff00112233".to_owned(),
        announce: None,
        announce_list: None,
    }
}

#[test]
fn verified_file_seeds_standard_blocks_and_reports_payload() {
    let root = tempdir().unwrap();
    let bytes = deterministic_bytes(300 * 1024 + 123);
    let source = root.path().join("seed.bin");
    fs::write(&source, &bytes).unwrap();
    let descriptor = create_descriptor(&source, ChunkProfile::named("s").unwrap()).unwrap();
    let torrent = torrent_for(&bytes, 128 * 1024);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_torrent = torrent.clone();
    let server_descriptor = descriptor.clone();
    let server_source = source.clone();
    let server = thread::spawn(move || {
        serve_v1_file_listener(
            listener,
            &server_torrent,
            &server_descriptor,
            &server_source,
            Some(1),
        )
        .unwrap()
    });

    let mut stream = TcpStream::connect(address).unwrap();
    send_handshake(&mut stream, &torrent);
    read_handshake(&mut stream, &torrent);
    let (message, bitfield) = read_message(&mut stream);
    assert_eq!(message, 5);
    assert_eq!(bitfield.len(), torrent.piece_sha1.len().div_ceil(8));
    send_message(&mut stream, 2, &[]);
    assert_eq!(read_message(&mut stream), (1, Vec::new()));

    let mut rebuilt = vec![0_u8; bytes.len()];
    let mut requests = 0_u64;
    for piece in 0..torrent.piece_sha1.len() {
        let piece_offset = piece as u64 * torrent.piece_length;
        let piece_length = torrent
            .piece_length
            .min(torrent.total_length - piece_offset);
        let mut begin = 0_u64;
        while begin < piece_length {
            let length = (16 * 1024_u64).min(piece_length - begin) as u32;
            let mut request = Vec::new();
            request.extend_from_slice(&(piece as u32).to_be_bytes());
            request.extend_from_slice(&(begin as u32).to_be_bytes());
            request.extend_from_slice(&length.to_be_bytes());
            send_message(&mut stream, 6, &request);
            let (message, payload) = read_message(&mut stream);
            assert_eq!(message, 7);
            assert_eq!(&payload[..4], &(piece as u32).to_be_bytes());
            assert_eq!(&payload[4..8], &(begin as u32).to_be_bytes());
            let absolute = piece_offset + begin;
            rebuilt[absolute as usize..absolute as usize + length as usize]
                .copy_from_slice(&payload[8..]);
            begin += u64::from(length);
            requests += 1;
        }
    }
    send_message(&mut stream, 3, &[]);
    let report = server.join().unwrap();

    assert_eq!(rebuilt, bytes);
    assert!(report.source_verified);
    assert_eq!(report.successful_handshakes, 1);
    assert_eq!(report.block_requests, requests);
    assert_eq!(report.payload_bytes_sent, bytes.len() as u64);
    assert_eq!(report.protocol_errors, 0);
}

#[test]
fn seed_refuses_corrupted_source_and_non_loopback_default() {
    let root = tempdir().unwrap();
    let bytes = deterministic_bytes(64 * 1024);
    let source = root.path().join("seed.bin");
    fs::write(&source, &bytes).unwrap();
    let descriptor = create_descriptor(&source, ChunkProfile::named("s").unwrap()).unwrap();
    let torrent = torrent_for(&bytes, 32 * 1024);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut corrupted = bytes.clone();
    corrupted[123] ^= 0xff;
    fs::write(&source, corrupted).unwrap();
    assert!(serve_v1_file_listener(listener, &torrent, &descriptor, &source, Some(0)).is_err());
    let bind: SocketAddr = "0.0.0.0:0".parse().unwrap();
    assert!(serve_v1_file(&torrent, &descriptor, &source, bind, false, Some(0)).is_err());
}

#[test]
fn index_seed_reconstructs_pieces_on_demand_from_separate_chunk_files() {
    let root = tempdir().unwrap();
    let bytes = deterministic_bytes(700 * 1024 + 321);
    let target = root.path().join("target.bin");
    fs::write(&target, &bytes).unwrap();
    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let materials = root.path().join("materials");
    fs::create_dir(&materials).unwrap();
    for (number, chunk) in descriptor.chunks.iter().enumerate() {
        let start = chunk.offset as usize;
        let end = start + chunk.length as usize;
        fs::write(
            materials.join(format!("material-{number:04}.bin")),
            &bytes[start..end],
        )
        .unwrap();
    }
    fs::remove_file(&target).unwrap();
    let database = root.path().join("index.db");
    let mut index = IndexDb::open(&database).unwrap();
    index.index_directory(&materials, profile).unwrap();
    drop(index);

    let torrent = torrent_for(&bytes, 128 * 1024);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_torrent = torrent.clone();
    let server_descriptor = descriptor.clone();
    let server_database = database.clone();
    let server = thread::spawn(move || {
        let index = IndexDb::open(&server_database).unwrap();
        serve_v1_index_listener(
            listener,
            &server_torrent,
            &server_descriptor,
            &index,
            &server_database,
            Some(1),
        )
        .unwrap()
    });

    let mut stream = TcpStream::connect(address).unwrap();
    send_handshake(&mut stream, &torrent);
    read_handshake(&mut stream, &torrent);
    let (message, bitfield) = read_message(&mut stream);
    assert_eq!(message, 5);
    assert_eq!(bitfield, vec![0xfc]);
    send_message(&mut stream, 2, &[]);
    assert_eq!(read_message(&mut stream), (1, Vec::new()));

    let mut rebuilt = vec![0_u8; bytes.len()];
    let mut requests = 0_u64;
    for piece in 0..torrent.piece_sha1.len() {
        let piece_offset = piece as u64 * torrent.piece_length;
        let piece_length = torrent
            .piece_length
            .min(torrent.total_length - piece_offset);
        let mut begin = 0_u64;
        while begin < piece_length {
            let length = (16 * 1024_u64).min(piece_length - begin) as u32;
            let mut request = Vec::new();
            request.extend_from_slice(&(piece as u32).to_be_bytes());
            request.extend_from_slice(&(begin as u32).to_be_bytes());
            request.extend_from_slice(&length.to_be_bytes());
            send_message(&mut stream, 6, &request);
            let (message, payload) = read_message(&mut stream);
            assert_eq!(message, 7);
            let absolute = piece_offset + begin;
            rebuilt[absolute as usize..absolute as usize + length as usize]
                .copy_from_slice(&payload[8..]);
            begin += u64::from(length);
            requests += 1;
        }
    }
    send_message(&mut stream, 3, &[]);
    let report = server.join().unwrap();

    assert_eq!(rebuilt, bytes);
    assert!(!target.exists());
    assert_eq!(report.advertised_pieces, torrent.piece_sha1.len() as u64);
    assert_eq!(report.block_requests, requests);
    assert_eq!(report.payload_bytes_sent, bytes.len() as u64);
    assert!(report.on_demand_local_chunks_read > 0);
    assert!(report.on_demand_local_bytes_read >= bytes.len() as u64);
    assert_eq!(report.protocol_errors, 0);
}

fn send_handshake(stream: &mut TcpStream, torrent: &TorrentV1) {
    let mut handshake = Vec::new();
    handshake.push(19);
    handshake.extend_from_slice(b"BitTorrent protocol");
    handshake.extend_from_slice(&[0_u8; 8]);
    handshake.extend_from_slice(&hex::decode(&torrent.info_hash_sha1).unwrap());
    handshake.extend_from_slice(b"-TEST00-123456789012");
    stream.write_all(&handshake).unwrap();
}

fn read_handshake(stream: &mut TcpStream, torrent: &TorrentV1) {
    let mut response = [0_u8; 68];
    stream.read_exact(&mut response).unwrap();
    assert_eq!(&response[1..20], b"BitTorrent protocol");
    assert_eq!(
        &response[28..48],
        hex::decode(&torrent.info_hash_sha1).unwrap()
    );
}

fn send_message(stream: &mut TcpStream, message: u8, payload: &[u8]) {
    stream
        .write_all(&((payload.len() + 1) as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&[message]).unwrap();
    stream.write_all(payload).unwrap();
}

fn read_message(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).unwrap();
    let mut message = vec![0_u8; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut message).unwrap();
    (message[0], message[1..].to_vec())
}
