use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use meld_core::{fetch_v1_metadata_from_peer, parse_v1_magnet};
use sha1::{Digest, Sha1};

#[test]
fn fetches_and_verifies_bep9_metadata_from_a_direct_peer() {
    let info = single_file_info(b"hello");
    let info_hash = hex::encode(Sha1::digest(&info));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let peer = listener.local_addr().unwrap();
    let server_info = info.clone();
    let server = thread::spawn(move || serve_metadata_once(listener, &server_info));
    let magnet = parse_v1_magnet(&format!(
        "magnet:?xt=urn:btih:{info_hash}&tr=http%3A%2F%2F127.0.0.1%3A45995%2Fannounce"
    ))
    .unwrap();

    let (torrent, report) = fetch_v1_metadata_from_peer(&magnet, peer).unwrap();
    server.join().unwrap();

    assert_eq!(torrent.name, "file.bin");
    assert_eq!(torrent.total_length, 5);
    assert_eq!(torrent.info_hash_sha1, info_hash);
    assert_eq!(
        torrent.announce.as_deref(),
        Some("http://127.0.0.1:45995/announce")
    );
    assert_eq!(report.remote_ut_metadata_id, 7);
    assert_eq!(report.metadata_bytes, info.len() as u64);
    assert_eq!(report.metadata_pieces, 1);
    assert!(report.info_hash_verified);
}

#[test]
fn rejects_bep9_metadata_that_does_not_match_the_magnet_hash() {
    let expected_info = single_file_info(b"hello");
    let wrong_info = single_file_info(b"world");
    let info_hash = hex::encode(Sha1::digest(&expected_info));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let peer = listener.local_addr().unwrap();
    let server = thread::spawn(move || serve_metadata_once(listener, &wrong_info));
    let magnet = parse_v1_magnet(&format!("magnet:?xt=urn:btih:{info_hash}")).unwrap();

    let error = fetch_v1_metadata_from_peer(&magnet, peer).unwrap_err();
    server.join().unwrap();

    assert!(format!("{error:#}").contains("info-hash mismatch"));
}

#[test]
fn reassembles_multiple_bep9_metadata_pieces() {
    let mut info = single_file_info(b"hello");
    info.pop();
    info.extend_from_slice(b"6:source17000:");
    info.extend(std::iter::repeat_n(b'x', 17_000));
    info.push(b'e');
    let info_hash = hex::encode(Sha1::digest(&info));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let peer = listener.local_addr().unwrap();
    let server_info = info.clone();
    let server = thread::spawn(move || serve_metadata_once(listener, &server_info));
    let magnet = parse_v1_magnet(&format!("magnet:?xt=urn:btih:{info_hash}")).unwrap();

    let (torrent, report) = fetch_v1_metadata_from_peer(&magnet, peer).unwrap();
    server.join().unwrap();

    assert_eq!(torrent.info_hash_sha1, info_hash);
    assert_eq!(report.metadata_bytes, info.len() as u64);
    assert_eq!(report.metadata_pieces, 2);
}

fn single_file_info(payload: &[u8]) -> Vec<u8> {
    let mut info = format!(
        "d6:lengthi{}e4:name8:file.bin12:piece lengthi16384e6:pieces20:",
        payload.len()
    )
    .into_bytes();
    info.extend_from_slice(&Sha1::digest(payload));
    info.push(b'e');
    info
}

fn serve_metadata_once(listener: TcpListener, info: &[u8]) {
    let (mut stream, _) = listener.accept().unwrap();
    let mut handshake = [0_u8; 68];
    stream.read_exact(&mut handshake).unwrap();
    assert_eq!(&handshake[1..20], b"BitTorrent protocol");
    assert_ne!(handshake[25] & 0x10, 0);

    let mut response = handshake;
    response[25] |= 0x10;
    response[48..68].copy_from_slice(b"-MOCK00-123456789012");
    stream.write_all(&response).unwrap();

    let (message_id, payload) = read_message(&mut stream);
    assert_eq!(message_id, 20);
    assert_eq!(payload[0], 0);
    assert!(payload[1..].windows(11).any(|part| part == b"ut_metadata"));

    let handshake = format!("d1:md11:ut_metadatai7ee13:metadata_sizei{}ee", info.len());
    send_extended(&mut stream, 0, handshake.as_bytes());

    for (piece, data) in info.chunks(16 * 1024).enumerate() {
        let (message_id, payload) = read_message(&mut stream);
        assert_eq!(message_id, 20);
        assert_eq!(payload[0], 7);
        assert!(payload[1..].windows(11).any(|part| part == b"msg_typei0e"));
        let expected_piece = format!("piecei{piece}e");
        assert!(
            payload[1..]
                .windows(expected_piece.len())
                .any(|part| part == expected_piece.as_bytes())
        );

        let header = format!(
            "d8:msg_typei1e5:piecei{piece}e10:total_sizei{}ee",
            info.len()
        );
        let mut body = header.into_bytes();
        body.extend_from_slice(data);
        send_extended(&mut stream, 1, &body);
    }
}

fn read_message(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).unwrap();
    let mut message = vec![0_u8; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut message).unwrap();
    (message[0], message[1..].to_vec())
}

fn send_extended(stream: &mut TcpStream, extension_id: u8, payload: &[u8]) {
    let length = u32::try_from(payload.len() + 2).unwrap();
    stream.write_all(&length.to_be_bytes()).unwrap();
    stream.write_all(&[20, extension_id]).unwrap();
    stream.write_all(payload).unwrap();
    stream.flush().unwrap();
}
