use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use meld_core::{
    ChunkProfile, IndexDb, TorrentV1, create_descriptor, fetch_v1_from_peer, fetch_v1_via_tracker,
    load_v1_torrent, parse_tracker_response, plan_v1_bridge,
};
use sha1::{Digest, Sha1};
use tempfile::tempdir;

fn deterministic_bytes(length: usize) -> Vec<u8> {
    let mut value = 0x6a09_e667_f3bc_c909_u64;
    let mut bytes = Vec::with_capacity(length);
    for _ in 0..length {
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        bytes.push((value >> 24) as u8);
    }
    bytes
}

fn single_file_torrent(name: &str, bytes: &[u8], piece_length: usize) -> Vec<u8> {
    single_file_torrent_with_announce(name, bytes, piece_length, None)
}

fn single_file_torrent_with_announce(
    name: &str,
    bytes: &[u8],
    piece_length: usize,
    announce: Option<&str>,
) -> Vec<u8> {
    single_file_torrent_with_trackers(name, bytes, piece_length, announce, None)
}

fn single_file_torrent_with_trackers(
    name: &str,
    bytes: &[u8],
    piece_length: usize,
    announce: Option<&str>,
    announce_list: Option<&[Vec<&str>]>,
) -> Vec<u8> {
    let mut piece_hashes = Vec::new();
    for piece in bytes.chunks(piece_length) {
        piece_hashes.extend_from_slice(&Sha1::digest(piece));
    }
    let mut torrent = Vec::new();
    torrent.push(b'd');
    if let Some(announce) = announce {
        torrent.extend_from_slice(b"8:announce");
        torrent.extend_from_slice(announce.len().to_string().as_bytes());
        torrent.push(b':');
        torrent.extend_from_slice(announce.as_bytes());
    }
    if let Some(tiers) = announce_list {
        torrent.extend_from_slice(b"13:announce-listl");
        for tier in tiers {
            torrent.push(b'l');
            for tracker in tier {
                torrent.extend_from_slice(tracker.len().to_string().as_bytes());
                torrent.push(b':');
                torrent.extend_from_slice(tracker.as_bytes());
            }
            torrent.push(b'e');
        }
        torrent.push(b'e');
    }
    torrent.extend_from_slice(b"4:info");
    torrent.extend_from_slice(b"d6:lengthi");
    torrent.extend_from_slice(bytes.len().to_string().as_bytes());
    torrent.extend_from_slice(b"e4:name");
    torrent.extend_from_slice(name.len().to_string().as_bytes());
    torrent.push(b':');
    torrent.extend_from_slice(name.as_bytes());
    torrent.extend_from_slice(b"12:piece lengthi");
    torrent.extend_from_slice(piece_length.to_string().as_bytes());
    torrent.extend_from_slice(b"e6:pieces");
    torrent.extend_from_slice(piece_hashes.len().to_string().as_bytes());
    torrent.push(b':');
    torrent.extend_from_slice(&piece_hashes);
    torrent.extend_from_slice(b"ee");
    torrent
}

#[test]
fn parses_multitracker_tiers_without_changing_info_hash() {
    let root = tempdir().unwrap();
    let bytes = deterministic_bytes(128 * 1024);
    let plain = root.path().join("plain.torrent");
    let tiered = root.path().join("tiered.torrent");
    fs::write(&plain, single_file_torrent("target.bin", &bytes, 64 * 1024)).unwrap();
    let tiers = vec![
        vec!["udp://127.0.0.1:45993/announce"],
        vec![
            "http://127.0.0.1:45994/announce",
            "https://tracker.example/announce",
        ],
    ];
    fs::write(
        &tiered,
        single_file_torrent_with_trackers(
            "target.bin",
            &bytes,
            64 * 1024,
            Some("http://ignored.example/announce"),
            Some(&tiers),
        ),
    )
    .unwrap();

    let plain = load_v1_torrent(&plain).unwrap();
    let tiered = load_v1_torrent(&tiered).unwrap();
    assert_eq!(plain.info_hash_sha1, tiered.info_hash_sha1);
    assert_eq!(tiered.announce_list.unwrap().len(), 2);
}

#[test]
fn parses_compact_ipv4_and_ipv6_tracker_peers() {
    let mut response = b"d8:intervali120e5:peers6:".to_vec();
    response.extend_from_slice(&[127, 0, 0, 1, 0xb3, 0x50]);
    response.extend_from_slice(b"6:peers618:");
    response.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    response.extend_from_slice(&[0xb3, 0x51]);
    response.push(b'e');

    let parsed = parse_tracker_response(&response).unwrap();
    assert_eq!(parsed.interval, 120);
    assert_eq!(parsed.peers[0].to_string(), "127.0.0.1:45904");
    assert_eq!(parsed.peers[1].to_string(), "[::1]:45905");
}

#[test]
fn parses_dictionary_tracker_peers_and_rejects_failure_reason() {
    let parsed =
        parse_tracker_response(b"d8:intervali30e5:peersld2:ip9:127.0.0.14:porti45906eeee").unwrap();
    assert_eq!(parsed.peers[0].to_string(), "127.0.0.1:45906");

    let error = parse_tracker_response(b"d14:failure reason11:not allowede").unwrap_err();
    assert!(error.to_string().contains("not allowed"));
}

#[test]
fn exact_local_file_maps_to_verified_bt_have_pieces() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let bytes = deterministic_bytes(512 * 1024 + 123);
    let target = sources.join("target.bin");
    fs::write(&target, &bytes).unwrap();
    let torrent_path = root.path().join("target.torrent");
    fs::write(
        &torrent_path,
        single_file_torrent("target.bin", &bytes, 64 * 1024),
    )
    .unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let torrent = load_v1_torrent(&torrent_path).unwrap();
    let report = plan_v1_bridge(&torrent, &descriptor, &index).unwrap();

    assert_eq!(report.fully_local_pieces, report.total_pieces);
    assert_eq!(report.fully_reconstructable_piece_bytes, bytes.len() as u64);
    assert_eq!(report.missing_bytes, 0);
    assert!(report.pieces.iter().all(|piece| piece.local_sha1_verified));
}

#[test]
fn shifted_target_reports_a_mixture_of_complete_and_incomplete_bt_pieces() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let original = deterministic_bytes(2 * 1024 * 1024);
    fs::write(sources.join("old.bin"), &original).unwrap();
    let mut target_bytes = original.clone();
    target_bytes.splice(600_000..600_000, deterministic_bytes(12_345));
    for byte in &mut target_bytes[1_400_000..1_420_000] {
        *byte ^= 0x5a;
    }
    let target = root.path().join("target.bin");
    fs::write(&target, &target_bytes).unwrap();
    let torrent_path = root.path().join("target.torrent");
    fs::write(
        &torrent_path,
        single_file_torrent("target.bin", &target_bytes, 64 * 1024),
    )
    .unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let report = plan_v1_bridge(
        &load_v1_torrent(&torrent_path).unwrap(),
        &descriptor,
        &index,
    )
    .unwrap();

    assert!(report.fully_local_pieces > 0);
    assert!(report.missing_pieces > 0);
    assert!(report.locally_covered_bytes > report.fully_reconstructable_piece_bytes);
    assert!(report.local_coverage_ratio > 0.80);
}

#[test]
fn locally_complete_piece_must_match_torrent_sha1() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let bytes = deterministic_bytes(128 * 1024);
    let target = sources.join("target.bin");
    fs::write(&target, &bytes).unwrap();
    let mut encoded = single_file_torrent("target.bin", &bytes, 64 * 1024);
    let first_hash_byte = encoded
        .windows(b"6:pieces40:".len())
        .position(|window| window == b"6:pieces40:")
        .unwrap()
        + b"6:pieces40:".len();
    encoded[first_hash_byte] ^= 0xff;
    let torrent_path = root.path().join("bad.torrent");
    fs::write(&torrent_path, encoded).unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let error = plan_v1_bridge(
        &load_v1_torrent(&torrent_path).unwrap(),
        &descriptor,
        &index,
    )
    .unwrap_err();
    assert!(error.to_string().contains("failed SHA-1"));
}

#[test]
fn rejects_multi_file_v1_torrent() {
    let root = tempdir().unwrap();
    let torrent = root.path().join("multi.torrent");
    fs::write(
        &torrent,
        b"d4:infod5:filesld6:lengthi1e4:pathl1:aeee4:name1:x12:piece lengthi1e6:pieces20:00000000000000000000ee",
    )
    .unwrap();
    let error = load_v1_torrent(&torrent).unwrap_err();
    assert!(error.to_string().contains("parse torrent"));
    assert!(format!("{error:#}").contains("multi-file"));
}

#[test]
fn downloads_only_missing_standard_blocks_from_peer_and_rebuilds() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let original = deterministic_bytes(2 * 1024 * 1024);
    fs::write(sources.join("old.bin"), &original).unwrap();
    let mut target_bytes = original.clone();
    target_bytes.splice(700_000..700_000, deterministic_bytes(19_321));
    for byte in &mut target_bytes[1_500_000..1_540_000] {
        *byte ^= 0x39;
    }
    let target = root.path().join("target.bin");
    fs::write(&target, &target_bytes).unwrap();
    let torrent_path = root.path().join("target.torrent");
    fs::write(
        &torrent_path,
        single_file_torrent("target.bin", &target_bytes, 256 * 1024),
    )
    .unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let torrent = load_v1_torrent(&torrent_path).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let peer_target = target_bytes.clone();
    let peer_torrent = torrent.clone();
    let server = thread::spawn(move || serve_standard_peer(listener, peer_torrent, peer_target));

    let output = root.path().join("rebuilt.bin");
    let report = fetch_v1_from_peer(&torrent, &descriptor, &index, address, &output).unwrap();
    let served = server.join().unwrap();

    assert!(report.verified);
    assert_eq!(fs::read(output).unwrap(), target_bytes);
    assert_eq!(report.network_block_requests, served.0);
    assert_eq!(report.network_payload_bytes, served.1);
    assert!(report.network_payload_bytes >= report.genuinely_missing_bytes);
    assert!(report.network_payload_bytes < report.target_bytes);
    assert!(report.local_bytes_available > report.network_payload_bytes);
}

#[test]
fn pipelines_out_of_order_blocks_and_resumes_only_sha1_verified_pieces() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let target_bytes = deterministic_bytes(1024 * 1024);
    let target = root.path().join("target.bin");
    fs::write(&target, &target_bytes).unwrap();
    let torrent_path = root.path().join("target.torrent");
    fs::write(
        &torrent_path,
        single_file_torrent("target.bin", &target_bytes, 256 * 1024),
    )
    .unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let torrent = load_v1_torrent(&torrent_path).unwrap();

    let interrupted_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let interrupted_address = interrupted_listener.local_addr().unwrap();
    let interrupted_target = target_bytes.clone();
    let interrupted_torrent = torrent.clone();
    let interrupted_peer = thread::spawn(move || {
        serve_one_pipelined_piece_then_disconnect(
            interrupted_listener,
            interrupted_torrent,
            interrupted_target,
        )
    });
    let output = root.path().join("resumed.bin");
    let error = fetch_v1_from_peer(&torrent, &descriptor, &index, interrupted_address, &output)
        .unwrap_err();
    assert!(!error.to_string().is_empty());
    assert_eq!(interrupted_peer.join().unwrap(), 16);
    assert!(!output.exists());

    let partial = root.path().join("resumed.bin.shardmeld-partial");
    let state = root.path().join("resumed.bin.shardmeld-resume.json");
    assert!(partial.exists());
    assert!(state.exists());

    let corrupted_output = root.path().join("corrupted-resume.bin");
    let corrupted_partial = root.path().join("corrupted-resume.bin.shardmeld-partial");
    let corrupted_state = root
        .path()
        .join("corrupted-resume.bin.shardmeld-resume.json");
    fs::copy(&partial, &corrupted_partial).unwrap();
    fs::copy(&state, &corrupted_state).unwrap();
    let mut corrupted = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&corrupted_partial)
        .unwrap();
    corrupted.seek(std::io::SeekFrom::Start(123)).unwrap();
    let mut byte = [0_u8; 1];
    corrupted.read_exact(&mut byte).unwrap();
    byte[0] ^= 0xff;
    corrupted.seek(std::io::SeekFrom::Start(123)).unwrap();
    corrupted.write_all(&byte).unwrap();
    corrupted.sync_all().unwrap();

    let resumed_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let resumed_address = resumed_listener.local_addr().unwrap();
    let resumed_target = target_bytes.clone();
    let resumed_torrent = torrent.clone();
    let resumed_peer = thread::spawn(move || {
        serve_standard_peer(resumed_listener, resumed_torrent, resumed_target)
    });
    let report =
        fetch_v1_from_peer(&torrent, &descriptor, &index, resumed_address, &output).unwrap();
    let resumed_served = resumed_peer.join().unwrap();
    assert_eq!(fs::read(&output).unwrap(), target_bytes);
    assert_eq!(report.request_window, 16);
    assert_eq!(report.resumed_verified_pieces, 1);
    assert_eq!(report.resumed_verified_piece_bytes, 256 * 1024);
    assert_eq!(report.network_payload_avoided_by_resume, 256 * 1024);
    assert_eq!(report.network_payload_bytes, 768 * 1024);
    assert_eq!(resumed_served.1, 768 * 1024);
    assert_eq!(report.newly_verified_pieces, 3);
    assert!(!partial.exists());
    assert!(!state.exists());

    let repaired_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let repaired_address = repaired_listener.local_addr().unwrap();
    let repaired_target = target_bytes.clone();
    let repaired_torrent = torrent.clone();
    let repaired_peer = thread::spawn(move || {
        serve_standard_peer(repaired_listener, repaired_torrent, repaired_target)
    });
    let repaired = fetch_v1_from_peer(
        &torrent,
        &descriptor,
        &index,
        repaired_address,
        &corrupted_output,
    )
    .unwrap();
    let repaired_served = repaired_peer.join().unwrap();
    assert_eq!(fs::read(corrupted_output).unwrap(), target_bytes);
    assert_eq!(repaired.resumed_verified_pieces, 0);
    assert_eq!(repaired.network_payload_avoided_by_resume, 0);
    assert_eq!(repaired.network_payload_bytes, 1024 * 1024);
    assert_eq!(repaired_served.1, 1024 * 1024);
}

#[test]
fn corrupted_standard_peer_data_never_persists_an_output() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let target_bytes = deterministic_bytes(512 * 1024);
    let target = root.path().join("target.bin");
    fs::write(&target, &target_bytes).unwrap();
    let torrent_path = root.path().join("target.torrent");
    fs::write(
        &torrent_path,
        single_file_torrent("target.bin", &target_bytes, 256 * 1024),
    )
    .unwrap();
    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let torrent = load_v1_torrent(&torrent_path).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let peer_torrent = torrent.clone();
    let server =
        thread::spawn(move || serve_corrupt_standard_peer(listener, peer_torrent, target_bytes));

    let output = root.path().join("must-not-exist.bin");
    let error = fetch_v1_from_peer(&torrent, &descriptor, &index, address, &output).unwrap_err();
    server.join().unwrap();
    assert!(error.to_string().contains("SHA-1 mismatch"));
    assert!(!output.exists());
}

#[test]
fn tracker_discovery_falls_back_to_second_peer_and_rebuilds() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let original = deterministic_bytes(1024 * 1024);
    fs::write(sources.join("old.bin"), &original).unwrap();
    let mut target_bytes = original.clone();
    target_bytes.splice(333_333..333_333, deterministic_bytes(17_003));
    for byte in &mut target_bytes[800_000..830_000] {
        *byte ^= 0x73;
    }
    let target = root.path().join("target.bin");
    fs::write(&target, &target_bytes).unwrap();

    let peer_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let peer_address = peer_listener.local_addr().unwrap();
    let unavailable_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let unavailable_address = unavailable_listener.local_addr().unwrap();
    drop(unavailable_listener);

    let tracker_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let tracker_address = tracker_listener.local_addr().unwrap();
    let announce = format!("http://{tracker_address}/announce");
    let torrent_path = root.path().join("target.torrent");
    fs::write(
        &torrent_path,
        single_file_torrent_with_announce("target.bin", &target_bytes, 256 * 1024, Some(&announce)),
    )
    .unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let torrent = load_v1_torrent(&torrent_path).unwrap();
    let peer_torrent = torrent.clone();
    let peer_target = target_bytes.clone();
    let peer = thread::spawn(move || serve_standard_peer(peer_listener, peer_torrent, peer_target));
    let tracker = thread::spawn(move || {
        serve_mock_tracker(tracker_listener, [unavailable_address, peer_address], 2)
    });

    let output = root.path().join("rebuilt-via-tracker.bin");
    let report = fetch_v1_via_tracker(&torrent, &descriptor, &index, None, 6881, &output).unwrap();
    peer.join().unwrap();
    let tracker_requests = tracker.join().unwrap();

    assert_eq!(fs::read(output).unwrap(), target_bytes);
    assert_eq!(report.peers_discovered, 2);
    assert_eq!(report.peers_attempted.len(), 2);
    assert!(report.peers_attempted[0].error.is_some());
    assert_eq!(report.selected_peer, peer_address);
    assert!(report.verified);
    assert!(tracker_requests[0].contains("event=started"));
    assert!(tracker_requests[0].contains("info_hash=%"));
    assert!(tracker_requests[1].contains("event=stopped"));
}

#[test]
fn tracker_prefers_rarest_pieces_across_two_peers() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let target_bytes = deterministic_bytes(1024 * 1024);
    let target = root.path().join("target.bin");
    fs::write(&target, &target_bytes).unwrap();

    let first_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let first_address = first_listener.local_addr().unwrap();
    let second_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let second_address = second_listener.local_addr().unwrap();
    let tracker_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let tracker_address = tracker_listener.local_addr().unwrap();
    let announce = format!("http://{tracker_address}/announce");
    let torrent_path = root.path().join("target.torrent");
    fs::write(
        &torrent_path,
        single_file_torrent_with_announce("target.bin", &target_bytes, 256 * 1024, Some(&announce)),
    )
    .unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let torrent = load_v1_torrent(&torrent_path).unwrap();
    let first_torrent = torrent.clone();
    let first_target = target_bytes.clone();
    let first = thread::spawn(move || {
        serve_selected_peer(
            first_listener,
            first_torrent,
            first_target,
            vec![true, true, true, false],
        )
    });
    let second_torrent = torrent.clone();
    let second_target = target_bytes.clone();
    let second = thread::spawn(move || {
        serve_selected_peer(
            second_listener,
            second_torrent,
            second_target,
            vec![false, true, true, true],
        )
    });
    let tracker = thread::spawn(move || {
        serve_mock_tracker(tracker_listener, [first_address, second_address], 2)
    });

    let output = root.path().join("multi-peer.bin");
    let report = fetch_v1_via_tracker(&torrent, &descriptor, &index, None, 6881, &output).unwrap();
    let first_served = first.join().unwrap();
    let second_served = second.join().unwrap();
    tracker.join().unwrap();

    assert_eq!(fs::read(output).unwrap(), target_bytes);
    assert_eq!(report.transfer.concurrent_peer_limit, 4);
    assert_eq!(report.transfer.peers_connected, 2);
    assert_eq!(report.transfer.contributing_peers.len(), 2);
    assert_eq!(report.transfer.pieces_reassigned, 0);
    assert!(report.transfer.endgame_enabled);
    assert!(report.transfer.endgame_duplicate_pieces >= 1);
    assert!(first_served.0 + second_served.0 >= 4);
    assert!(first_served.1 + second_served.1 >= target_bytes.len() as u64);
    let first_attempt = report
        .peers_attempted
        .iter()
        .find(|attempt| attempt.peer == first_address)
        .unwrap();
    let second_attempt = report
        .peers_attempted
        .iter()
        .find(|attempt| attempt.peer == second_address)
        .unwrap();
    assert_eq!(first_attempt.verified_piece_indices[0], 0);
    assert_eq!(second_attempt.verified_piece_indices[0], 3);
    assert_eq!(
        first_attempt.pieces_verified + second_attempt.pieces_verified,
        4
    );
    assert!(first_attempt.connected && second_attempt.connected);
}

#[test]
fn faster_peer_automatically_claims_more_piece_jobs() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let target_bytes = deterministic_bytes(2 * 1024 * 1024);
    let target = root.path().join("target.bin");
    fs::write(&target, &target_bytes).unwrap();

    let slow_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let slow_address = slow_listener.local_addr().unwrap();
    let fast_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let fast_address = fast_listener.local_addr().unwrap();
    let tracker_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let tracker_address = tracker_listener.local_addr().unwrap();
    let announce = format!("http://{tracker_address}/announce");
    let torrent_path = root.path().join("target.torrent");
    fs::write(
        &torrent_path,
        single_file_torrent_with_announce("target.bin", &target_bytes, 256 * 1024, Some(&announce)),
    )
    .unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let torrent = load_v1_torrent(&torrent_path).unwrap();
    let slow_torrent = torrent.clone();
    let slow_target = target_bytes.clone();
    let slow = thread::spawn(move || {
        serve_delayed_peer(
            slow_listener,
            slow_torrent,
            slow_target,
            std::time::Duration::from_millis(5),
        )
    });
    let fast_torrent = torrent.clone();
    let fast_target = target_bytes.clone();
    let fast = thread::spawn(move || {
        serve_delayed_peer(
            fast_listener,
            fast_torrent,
            fast_target,
            std::time::Duration::ZERO,
        )
    });
    let tracker = thread::spawn(move || {
        serve_mock_tracker(tracker_listener, [slow_address, fast_address], 2)
    });

    let output = root.path().join("adaptive.bin");
    let report = fetch_v1_via_tracker(&torrent, &descriptor, &index, None, 6881, &output).unwrap();
    slow.join().unwrap();
    fast.join().unwrap();
    tracker.join().unwrap();

    assert_eq!(fs::read(output).unwrap(), target_bytes);
    let slow_attempt = report
        .peers_attempted
        .iter()
        .find(|attempt| attempt.peer == slow_address)
        .unwrap();
    let fast_attempt = report
        .peers_attempted
        .iter()
        .find(|attempt| attempt.peer == fast_address)
        .unwrap();
    assert!(fast_attempt.pieces_verified > slow_attempt.pieces_verified);
    assert!(fast_attempt.payload_bytes_received > slow_attempt.payload_bytes_received);
    assert!(fast_attempt.payload_bytes_per_second > slow_attempt.payload_bytes_per_second);
    assert_eq!(
        fast_attempt.pieces_verified + slow_attempt.pieces_verified,
        8
    );
}

#[test]
fn endgame_sends_cancel_for_losing_duplicate_requests() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let target_bytes = deterministic_bytes(512 * 1024);
    let target = root.path().join("target.bin");
    fs::write(&target, &target_bytes).unwrap();

    let slow_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let slow_address = slow_listener.local_addr().unwrap();
    let fast_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let fast_address = fast_listener.local_addr().unwrap();
    let tracker_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let tracker_address = tracker_listener.local_addr().unwrap();
    let announce = format!("http://{tracker_address}/announce");
    let torrent_path = root.path().join("target.torrent");
    fs::write(
        &torrent_path,
        single_file_torrent_with_announce("target.bin", &target_bytes, 256 * 1024, Some(&announce)),
    )
    .unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let torrent = load_v1_torrent(&torrent_path).unwrap();
    let (slow_ready_tx, slow_ready_rx) = mpsc::channel();
    let slow_torrent = torrent.clone();
    let slow_target = target_bytes.clone();
    let slow = thread::spawn(move || {
        serve_endgame_slow_peer(slow_listener, slow_torrent, slow_target, slow_ready_tx)
    });
    let fast_torrent = torrent.clone();
    let fast_target = target_bytes.clone();
    let fast = thread::spawn(move || {
        serve_endgame_fast_peer(fast_listener, fast_torrent, fast_target, slow_ready_rx)
    });
    let tracker = thread::spawn(move || {
        serve_mock_tracker(tracker_listener, [slow_address, fast_address], 2)
    });

    let output = root.path().join("endgame.bin");
    let report = fetch_v1_via_tracker(&torrent, &descriptor, &index, None, 6881, &output).unwrap();
    let cancel_messages = slow.join().unwrap();
    fast.join().unwrap();
    tracker.join().unwrap();

    assert_eq!(fs::read(output).unwrap(), target_bytes);
    assert_eq!(report.transfer.newly_verified_pieces, 2);
    assert_eq!(report.transfer.endgame_duplicate_pieces, 1);
    assert_eq!(report.transfer.endgame_cancelled_jobs, 1);
    assert_eq!(report.transfer.endgame_cancel_messages, 15);
    assert_eq!(cancel_messages, 15);
    let slow_attempt = report
        .peers_attempted
        .iter()
        .find(|attempt| attempt.peer == slow_address)
        .unwrap();
    assert_eq!(slow_attempt.cancel_messages_sent, 15);
    assert_eq!(slow_attempt.endgame_jobs_cancelled, 1);
}

#[test]
fn endgame_finishes_around_a_stalled_peer_without_republishing_its_piece() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let target_bytes = deterministic_bytes(1024 * 1024);
    let target = root.path().join("target.bin");
    fs::write(&target, &target_bytes).unwrap();

    let stalled_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let stalled_address = stalled_listener.local_addr().unwrap();
    let good_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let good_address = good_listener.local_addr().unwrap();
    let tracker_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let tracker_address = tracker_listener.local_addr().unwrap();
    let announce = format!("http://{tracker_address}/announce");
    let torrent_path = root.path().join("target.torrent");
    fs::write(
        &torrent_path,
        single_file_torrent_with_announce("target.bin", &target_bytes, 256 * 1024, Some(&announce)),
    )
    .unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let torrent = load_v1_torrent(&torrent_path).unwrap();
    let stalled_torrent = torrent.clone();
    let stalled_target = target_bytes.clone();
    let stalled = thread::spawn(move || {
        serve_stalled_peer(stalled_listener, stalled_torrent, stalled_target)
    });
    let good_torrent = torrent.clone();
    let good_target = target_bytes.clone();
    let good = thread::spawn(move || serve_standard_peer(good_listener, good_torrent, good_target));
    let tracker = thread::spawn(move || {
        serve_mock_tracker(tracker_listener, [stalled_address, good_address], 2)
    });

    let output = root.path().join("reassigned.bin");
    let report = fetch_v1_via_tracker(&torrent, &descriptor, &index, None, 6881, &output).unwrap();
    stalled.join().unwrap();
    let good_served = good.join().unwrap();
    tracker.join().unwrap();

    assert_eq!(fs::read(output).unwrap(), target_bytes);
    assert_eq!(report.transfer.peers_connected, 2);
    assert_eq!(report.transfer.pieces_reassigned, 0);
    assert!(report.transfer.endgame_duplicate_pieces >= 1);
    assert_eq!(report.transfer.endgame_cancelled_jobs, 1);
    assert_eq!(report.transfer.contributing_peers, vec![good_address]);
    assert_eq!(good_served.1, target_bytes.len() as u64);
    assert_eq!(
        report.transfer.network_payload_bytes,
        target_bytes.len() as u64 + 16 * 1024
    );
    assert_eq!(report.transfer.network_redundant_bytes, 16 * 1024);
    assert_eq!(report.transfer.network_block_requests, 80);
    let stalled_attempt = report
        .peers_attempted
        .iter()
        .find(|attempt| attempt.peer == stalled_address)
        .unwrap();
    assert!(stalled_attempt.error.is_none());
    assert_eq!(stalled_attempt.endgame_jobs_cancelled, 1);
    assert_eq!(stalled_attempt.block_requests_sent, 16);
    assert_eq!(stalled_attempt.payload_bytes_received, 16 * 1024);
}

#[test]
fn multitracker_falls_back_to_udp_tier_and_rebuilds() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let original = deterministic_bytes(1024 * 1024);
    fs::write(sources.join("old.bin"), &original).unwrap();
    let mut target_bytes = original.clone();
    target_bytes.splice(222_222..222_222, deterministic_bytes(13_337));
    for byte in &mut target_bytes[700_000..728_000] {
        *byte ^= 0x42;
    }
    let target = root.path().join("target.bin");
    fs::write(&target, &target_bytes).unwrap();

    let peer_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let peer_address = peer_listener.local_addr().unwrap();
    let unavailable_http = TcpListener::bind("127.0.0.1:0").unwrap();
    let unavailable_http_address = unavailable_http.local_addr().unwrap();
    drop(unavailable_http);
    let tracker_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let tracker_address = tracker_socket.local_addr().unwrap();
    let udp_url = format!("udp://{tracker_address}/announce");
    let http_url = format!("http://{unavailable_http_address}/announce");
    let tiers = vec![vec![http_url.as_str()], vec![udp_url.as_str()]];
    let torrent_path = root.path().join("target.torrent");
    fs::write(
        &torrent_path,
        single_file_torrent_with_trackers(
            "target.bin",
            &target_bytes,
            256 * 1024,
            Some("http://ignored.example/announce"),
            Some(&tiers),
        ),
    )
    .unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let torrent = load_v1_torrent(&torrent_path).unwrap();
    let peer_torrent = torrent.clone();
    let peer_target = target_bytes.clone();
    let peer = thread::spawn(move || serve_standard_peer(peer_listener, peer_torrent, peer_target));
    let expected_info_hash = hex::decode(&torrent.info_hash_sha1).unwrap();
    let tracker = thread::spawn(move || {
        serve_mock_udp_tracker(tracker_socket, peer_address, &expected_info_hash)
    });

    let output = root.path().join("rebuilt-via-udp-tracker.bin");
    let report = fetch_v1_via_tracker(&torrent, &descriptor, &index, None, 6881, &output).unwrap();
    peer.join().unwrap();
    let events = tracker.join().unwrap();

    assert_eq!(fs::read(output).unwrap(), target_bytes);
    assert_eq!(report.tracker_attempts.len(), 2);
    assert!(report.tracker_attempts[0].error.is_some());
    assert_eq!(report.tracker_attempts[1].tracker, udp_url);
    assert_eq!(report.selected_peer, peer_address);
    assert_eq!(events, vec![2, 3]);
    assert!(report.verified);
}

fn serve_mock_udp_tracker(
    socket: UdpSocket,
    peer: SocketAddr,
    expected_info_hash: &[u8],
) -> Vec<u32> {
    const CONNECTION_ID: u64 = 0x0102_0304_0506_0708;
    let mut events = Vec::new();
    for _ in 0..2 {
        let mut buffer = [0_u8; 2048];
        let (length, client) = socket.recv_from(&mut buffer).unwrap();
        assert_eq!(length, 16);
        assert_eq!(
            u64::from_be_bytes(buffer[..8].try_into().unwrap()),
            0x0417_2710_1980
        );
        assert_eq!(u32::from_be_bytes(buffer[8..12].try_into().unwrap()), 0);
        let transaction = u32::from_be_bytes(buffer[12..16].try_into().unwrap());
        let mut connect_response = Vec::new();
        connect_response.extend_from_slice(&0_u32.to_be_bytes());
        connect_response.extend_from_slice(&transaction.to_be_bytes());
        connect_response.extend_from_slice(&CONNECTION_ID.to_be_bytes());
        socket.send_to(&connect_response, client).unwrap();

        let (length, client) = socket.recv_from(&mut buffer).unwrap();
        assert_eq!(length, 98);
        assert_eq!(
            u64::from_be_bytes(buffer[..8].try_into().unwrap()),
            CONNECTION_ID
        );
        assert_eq!(u32::from_be_bytes(buffer[8..12].try_into().unwrap()), 1);
        assert_eq!(&buffer[16..36], expected_info_hash);
        let transaction = u32::from_be_bytes(buffer[12..16].try_into().unwrap());
        let event = u32::from_be_bytes(buffer[80..84].try_into().unwrap());
        events.push(event);
        let mut announce_response = Vec::new();
        announce_response.extend_from_slice(&1_u32.to_be_bytes());
        announce_response.extend_from_slice(&transaction.to_be_bytes());
        announce_response.extend_from_slice(&60_u32.to_be_bytes());
        announce_response.extend_from_slice(&0_u32.to_be_bytes());
        announce_response.extend_from_slice(&1_u32.to_be_bytes());
        if event == 2 {
            let std::net::IpAddr::V4(ip) = peer.ip() else {
                panic!("mock UDP tracker expects an IPv4 peer");
            };
            announce_response.extend_from_slice(&ip.octets());
            announce_response.extend_from_slice(&peer.port().to_be_bytes());
        }
        socket.send_to(&announce_response, client).unwrap();
    }
    events
}

fn serve_mock_tracker(
    listener: TcpListener,
    peers: [std::net::SocketAddr; 2],
    requests: usize,
) -> Vec<String> {
    let mut received = Vec::new();
    for request_index in 0..requests {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 2048];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..count]);
        }
        received.push(String::from_utf8_lossy(&request).into_owned());

        let mut body = b"d8:intervali60e5:peers".to_vec();
        if request_index == 0 {
            body.extend_from_slice(b"12:");
            for peer in peers {
                let std::net::IpAddr::V4(ip) = peer.ip() else {
                    panic!("mock tracker expects IPv4 peers");
                };
                body.extend_from_slice(&ip.octets());
                body.extend_from_slice(&peer.port().to_be_bytes());
            }
        } else {
            body.extend_from_slice(b"0:");
        }
        body.push(b'e');
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(&body).unwrap();
        stream.flush().unwrap();
    }
    received
}

fn serve_standard_peer(listener: TcpListener, torrent: TorrentV1, target: Vec<u8>) -> (u64, u64) {
    let (mut stream, _) = listener.accept().unwrap();
    peer_handshake(&mut stream, &torrent);
    let (message, payload) = read_peer_message(&mut stream);
    assert_eq!(message, 2);
    assert!(payload.is_empty());
    send_peer_bitfield(&mut stream, torrent.piece_sha1.len());
    send_peer_message(&mut stream, 1, &[]);

    let mut requests = 0_u64;
    let mut bytes = 0_u64;
    while let Ok((message, payload)) = try_read_peer_message(&mut stream) {
        if message == 3 {
            break;
        }
        assert_eq!(message, 6);
        assert_eq!(payload.len(), 12);
        let piece = u32::from_be_bytes(payload[0..4].try_into().unwrap());
        let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
        let length = u32::from_be_bytes(payload[8..12].try_into().unwrap());
        assert!(length <= 16 * 1024);
        let absolute = u64::from(piece) * torrent.piece_length + u64::from(begin);
        let end = absolute + u64::from(length);
        let block = &target[absolute as usize..end as usize];
        let mut response = Vec::with_capacity(8 + block.len());
        response.extend_from_slice(&piece.to_be_bytes());
        response.extend_from_slice(&begin.to_be_bytes());
        response.extend_from_slice(block);
        send_peer_message(&mut stream, 7, &response);
        requests += 1;
        bytes += u64::from(length);
    }
    (requests, bytes)
}

fn serve_endgame_fast_peer(
    listener: TcpListener,
    torrent: TorrentV1,
    target: Vec<u8>,
    slow_piece_ready: Receiver<()>,
) {
    let (mut stream, _) = listener.accept().unwrap();
    peer_handshake(&mut stream, &torrent);
    let (message, payload) = read_peer_message(&mut stream);
    assert_eq!(message, 2);
    assert!(payload.is_empty());
    send_peer_bitfield(&mut stream, torrent.piece_sha1.len());
    send_peer_message(&mut stream, 1, &[]);

    let mut waited_for_slow_piece = false;
    while let Ok((message, payload)) = try_read_peer_message(&mut stream) {
        if message == 3 {
            break;
        }
        assert_eq!(message, 6);
        assert_eq!(payload.len(), 12);
        let piece = u32::from_be_bytes(payload[0..4].try_into().unwrap());
        let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
        let length = u32::from_be_bytes(payload[8..12].try_into().unwrap());
        if piece == 0 && !waited_for_slow_piece {
            slow_piece_ready
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            waited_for_slow_piece = true;
        }
        let absolute = u64::from(piece) * torrent.piece_length + u64::from(begin);
        let block = &target[absolute as usize..absolute as usize + length as usize];
        let mut response = Vec::with_capacity(8 + block.len());
        response.extend_from_slice(&piece.to_be_bytes());
        response.extend_from_slice(&begin.to_be_bytes());
        response.extend_from_slice(block);
        if try_send_peer_message(&mut stream, 7, &response).is_err() {
            break;
        }
    }
}

fn serve_delayed_peer(
    listener: TcpListener,
    torrent: TorrentV1,
    target: Vec<u8>,
    response_delay: std::time::Duration,
) -> u64 {
    let (mut stream, _) = listener.accept().unwrap();
    peer_handshake(&mut stream, &torrent);
    let (message, payload) = read_peer_message(&mut stream);
    assert_eq!(message, 2);
    assert!(payload.is_empty());
    send_peer_bitfield(&mut stream, torrent.piece_sha1.len());
    send_peer_message(&mut stream, 1, &[]);
    let mut cancel_messages = 0_u64;
    while let Ok((message, payload)) = try_read_peer_message(&mut stream) {
        if message == 3 {
            break;
        }
        if message == 8 {
            assert_eq!(payload.len(), 12);
            cancel_messages += 1;
            continue;
        }
        assert_eq!(message, 6);
        let piece = u32::from_be_bytes(payload[0..4].try_into().unwrap());
        let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
        let length = u32::from_be_bytes(payload[8..12].try_into().unwrap());
        thread::sleep(response_delay);
        let absolute = u64::from(piece) * torrent.piece_length + u64::from(begin);
        let block = &target[absolute as usize..absolute as usize + length as usize];
        let mut response = Vec::with_capacity(8 + block.len());
        response.extend_from_slice(&piece.to_be_bytes());
        response.extend_from_slice(&begin.to_be_bytes());
        response.extend_from_slice(block);
        if try_send_peer_message(&mut stream, 7, &response).is_err() {
            break;
        }
    }
    cancel_messages
}

fn serve_selected_peer(
    listener: TcpListener,
    torrent: TorrentV1,
    target: Vec<u8>,
    availability: Vec<bool>,
) -> (u64, u64) {
    let (mut stream, _) = listener.accept().unwrap();
    peer_handshake(&mut stream, &torrent);
    let (message, payload) = read_peer_message(&mut stream);
    assert_eq!(message, 2);
    assert!(payload.is_empty());
    send_peer_selected_bitfield(&mut stream, &availability);
    send_peer_message(&mut stream, 1, &[]);

    let mut pieces = std::collections::HashSet::new();
    let mut bytes = 0_u64;
    while let Ok((message, payload)) = try_read_peer_message(&mut stream) {
        if message == 3 {
            break;
        }
        assert_eq!(message, 6);
        assert_eq!(payload.len(), 12);
        let piece = u32::from_be_bytes(payload[0..4].try_into().unwrap());
        let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
        let length = u32::from_be_bytes(payload[8..12].try_into().unwrap());
        assert!(availability[piece as usize]);
        pieces.insert(piece);
        let absolute = u64::from(piece) * torrent.piece_length + u64::from(begin);
        let block = &target[absolute as usize..absolute as usize + length as usize];
        let mut response = Vec::with_capacity(8 + block.len());
        response.extend_from_slice(&piece.to_be_bytes());
        response.extend_from_slice(&begin.to_be_bytes());
        response.extend_from_slice(block);
        if try_send_peer_message(&mut stream, 7, &response).is_err() {
            break;
        }
        bytes += u64::from(length);
    }
    (pieces.len() as u64, bytes)
}

fn serve_stalled_peer(listener: TcpListener, torrent: TorrentV1, target: Vec<u8>) {
    let (mut stream, _) = listener.accept().unwrap();
    peer_handshake(&mut stream, &torrent);
    let (message, payload) = read_peer_message(&mut stream);
    assert_eq!(message, 2);
    assert!(payload.is_empty());
    send_peer_bitfield(&mut stream, torrent.piece_sha1.len());
    send_peer_message(&mut stream, 1, &[]);
    let (message, payload) = read_peer_message(&mut stream);
    assert_eq!(message, 6);
    assert_eq!(payload.len(), 12);
    let piece = u32::from_be_bytes(payload[0..4].try_into().unwrap());
    let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
    let length = u32::from_be_bytes(payload[8..12].try_into().unwrap());
    let absolute = u64::from(piece) * torrent.piece_length + u64::from(begin);
    let block = &target[absolute as usize..absolute as usize + length as usize];
    let mut response = Vec::with_capacity(8 + block.len());
    response.extend_from_slice(&piece.to_be_bytes());
    response.extend_from_slice(&begin.to_be_bytes());
    response.extend_from_slice(block);
    send_peer_message(&mut stream, 7, &response);
    thread::sleep(std::time::Duration::from_secs(6));
}

fn serve_endgame_slow_peer(
    listener: TcpListener,
    torrent: TorrentV1,
    target: Vec<u8>,
    ready: Sender<()>,
) -> u64 {
    let (mut stream, _) = listener.accept().unwrap();
    peer_handshake(&mut stream, &torrent);
    let (message, payload) = read_peer_message(&mut stream);
    assert_eq!(message, 2);
    assert!(payload.is_empty());
    send_peer_selected_bitfield(&mut stream, &[true, false]);
    send_peer_message(&mut stream, 1, &[]);

    let mut requests = Vec::new();
    for _ in 0..16 {
        let (message, payload) = read_peer_message(&mut stream);
        assert_eq!(message, 6);
        assert_eq!(payload.len(), 12);
        requests.push((
            u32::from_be_bytes(payload[0..4].try_into().unwrap()),
            u32::from_be_bytes(payload[4..8].try_into().unwrap()),
            u32::from_be_bytes(payload[8..12].try_into().unwrap()),
        ));
    }
    let (piece, begin, length) = requests[0];
    assert_eq!(piece, 0);
    ready.send(()).unwrap();
    thread::sleep(std::time::Duration::from_millis(50));
    let absolute = u64::from(piece) * torrent.piece_length + u64::from(begin);
    let block = &target[absolute as usize..absolute as usize + length as usize];
    let mut response = Vec::with_capacity(8 + block.len());
    response.extend_from_slice(&piece.to_be_bytes());
    response.extend_from_slice(&begin.to_be_bytes());
    response.extend_from_slice(block);
    send_peer_message(&mut stream, 7, &response);

    let mut cancellations = 0_u64;
    while let Ok((message, payload)) = try_read_peer_message(&mut stream) {
        match message {
            8 => {
                assert_eq!(payload.len(), 12);
                cancellations += 1;
                if cancellations == 15 {
                    break;
                }
            }
            3 => break,
            _ => panic!("unexpected Endgame peer message {message}"),
        }
    }
    cancellations
}

fn serve_one_pipelined_piece_then_disconnect(
    listener: TcpListener,
    torrent: TorrentV1,
    target: Vec<u8>,
) -> usize {
    let (mut stream, _) = listener.accept().unwrap();
    peer_handshake(&mut stream, &torrent);
    let (message, payload) = read_peer_message(&mut stream);
    assert_eq!(message, 2);
    assert!(payload.is_empty());
    send_peer_bitfield(&mut stream, torrent.piece_sha1.len());
    send_peer_message(&mut stream, 1, &[]);

    let mut requests = Vec::new();
    for _ in 0..16 {
        let (message, payload) = read_peer_message(&mut stream);
        assert_eq!(message, 6);
        assert_eq!(payload.len(), 12);
        let piece = u32::from_be_bytes(payload[0..4].try_into().unwrap());
        let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
        let length = u32::from_be_bytes(payload[8..12].try_into().unwrap());
        assert_eq!(piece, 0);
        assert_eq!(length, 16 * 1024);
        requests.push((piece, begin, length));
    }
    for (piece, begin, length) in requests.iter().rev() {
        let absolute = u64::from(*piece) * torrent.piece_length + u64::from(*begin);
        let block = &target[absolute as usize..absolute as usize + *length as usize];
        let mut response = Vec::with_capacity(8 + block.len());
        response.extend_from_slice(&piece.to_be_bytes());
        response.extend_from_slice(&begin.to_be_bytes());
        response.extend_from_slice(block);
        send_peer_message(&mut stream, 7, &response);
    }
    stream.shutdown(Shutdown::Write).unwrap();
    while try_read_peer_message(&mut stream).is_ok() {}
    requests.len()
}

fn serve_corrupt_standard_peer(listener: TcpListener, torrent: TorrentV1, target: Vec<u8>) {
    let (mut stream, _) = listener.accept().unwrap();
    peer_handshake(&mut stream, &torrent);
    let _ = read_peer_message(&mut stream);
    send_peer_bitfield(&mut stream, torrent.piece_sha1.len());
    send_peer_message(&mut stream, 1, &[]);
    let mut corrupted_once = false;
    while let Ok((message, payload)) = try_read_peer_message(&mut stream) {
        if message == 3 {
            break;
        }
        if message != 6 || payload.len() != 12 {
            continue;
        }
        let piece = u32::from_be_bytes(payload[0..4].try_into().unwrap());
        let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
        let length = u32::from_be_bytes(payload[8..12].try_into().unwrap());
        let absolute = u64::from(piece) * torrent.piece_length + u64::from(begin);
        let mut block = target[absolute as usize..absolute as usize + length as usize].to_vec();
        if !corrupted_once {
            block[0] ^= 0xff;
            corrupted_once = true;
        }
        let mut response = Vec::with_capacity(8 + block.len());
        response.extend_from_slice(&piece.to_be_bytes());
        response.extend_from_slice(&begin.to_be_bytes());
        response.extend_from_slice(&block);
        send_peer_message(&mut stream, 7, &response);
    }
}

fn peer_handshake(stream: &mut TcpStream, torrent: &TorrentV1) {
    let mut handshake = [0_u8; 68];
    stream.read_exact(&mut handshake).unwrap();
    assert_eq!(handshake[0], 19);
    assert_eq!(&handshake[1..20], b"BitTorrent protocol");
    assert_eq!(
        &handshake[28..48],
        &hex::decode(&torrent.info_hash_sha1).unwrap()
    );
    let mut response = Vec::with_capacity(68);
    response.push(19);
    response.extend_from_slice(b"BitTorrent protocol");
    response.extend_from_slice(&[0_u8; 8]);
    response.extend_from_slice(&handshake[28..48]);
    response.extend_from_slice(b"-MOCK00-123456789012");
    stream.write_all(&response).unwrap();
    stream.flush().unwrap();
}

fn send_peer_bitfield(stream: &mut TcpStream, piece_count: usize) {
    send_peer_selected_bitfield(stream, &vec![true; piece_count]);
}

fn send_peer_selected_bitfield(stream: &mut TcpStream, availability: &[bool]) {
    let mut bitfield = vec![0_u8; availability.len().div_ceil(8)];
    for (index, available) in availability.iter().enumerate() {
        if *available {
            bitfield[index / 8] |= 0x80 >> (index % 8);
        }
    }
    send_peer_message(stream, 5, &bitfield);
}

fn send_peer_message(stream: &mut TcpStream, message: u8, payload: &[u8]) {
    try_send_peer_message(stream, message, payload).unwrap();
}

fn try_send_peer_message(
    stream: &mut TcpStream,
    message: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    let length = (payload.len() as u32 + 1).to_be_bytes();
    stream.write_all(&length)?;
    stream.write_all(&[message])?;
    stream.write_all(payload)?;
    stream.flush()
}

fn read_peer_message(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    try_read_peer_message(stream).unwrap()
}

fn try_read_peer_message(stream: &mut TcpStream) -> std::io::Result<(u8, Vec<u8>)> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    let mut message = vec![0_u8; length];
    stream.read_exact(&mut message)?;
    Ok((message[0], message[1..].to_vec()))
}
