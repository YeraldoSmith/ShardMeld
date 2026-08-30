use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::thread;

use meld_core::{
    ChunkProfile, IndexDb, compare_descriptor, create_descriptor, fetch_missing_chunks,
    rebuild_target, serve_chunk_directory, serve_chunk_listener, stage_missing_chunks,
    verify_target,
};
use tempfile::tempdir;

fn deterministic_bytes(length: usize) -> Vec<u8> {
    let mut value = 0x1234_5678_9abc_def0_u64;
    let mut bytes = Vec::with_capacity(length);
    for _ in 0..length {
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        bytes.push((value >> 24) as u8);
    }
    bytes
}

#[test]
fn reconstructs_shifted_and_modified_target_exactly() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    let missing = root.path().join("missing");
    fs::create_dir_all(&sources).unwrap();

    let base = deterministic_bytes(4 * 1024 * 1024);
    fs::write(sources.join("base.bin"), &base).unwrap();

    let mut target = base.clone();
    target.splice(700_000..700_000, deterministic_bytes(12_345));
    for byte in &mut target[2_000_000..2_032_768] {
        *byte ^= 0x5a;
    }
    let target_path = root.path().join("target.bin");
    fs::write(&target_path, &target).unwrap();

    let profile = ChunkProfile::named("m").unwrap();
    let descriptor = create_descriptor(&target_path, profile).unwrap();
    let database = root.path().join("index.db");
    let mut index = IndexDb::open(&database).unwrap();
    let index_report = index.index_directory(&sources, profile).unwrap();
    assert_eq!(index_report.files_indexed, 1);
    assert_eq!(index_report.bytes_indexed, base.len() as u64);

    let compare = compare_descriptor(&descriptor, &index).unwrap();
    assert!(compare.reuse_ratio > 0.80, "reuse={}", compare.reuse_ratio);
    assert!(compare.reuse_ratio < 1.0);
    assert_eq!(
        compare.local_reusable_bytes + compare.missing_payload_bytes,
        descriptor.target.size
    );

    let staged = stage_missing_chunks(&descriptor, &target_path, &index, &missing).unwrap();
    assert_eq!(staged.missing_payload_bytes, compare.missing_payload_bytes);

    let rebuilt = root.path().join("rebuilt.bin");
    let report = rebuild_target(&descriptor, &index, &missing, &rebuilt).unwrap();
    assert!(report.verified);
    assert_eq!(report.local_bytes_read, compare.local_reusable_bytes);
    assert_eq!(
        report.missing_source_bytes_read,
        compare.missing_payload_bytes
    );
    assert_eq!(fs::read(&rebuilt).unwrap(), target);
    assert!(verify_target(&descriptor, &rebuilt).unwrap().verified);
}

#[test]
fn identical_file_reuses_every_byte() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let bytes = deterministic_bytes(1024 * 1024);
    let source = sources.join("same.bin");
    fs::write(&source, &bytes).unwrap();

    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&source, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let report = compare_descriptor(&descriptor, &index).unwrap();
    assert_eq!(report.reuse_ratio, 1.0);
    assert_eq!(report.missing_payload_bytes, 0);
}

#[test]
fn stale_source_is_not_reported_as_reusable() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let source = sources.join("source.bin");
    fs::write(&source, deterministic_bytes(1024 * 1024)).unwrap();
    let profile = ChunkProfile::named("m").unwrap();
    let descriptor = create_descriptor(&source, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    assert_eq!(
        compare_descriptor(&descriptor, &index).unwrap().reuse_ratio,
        1.0
    );

    let mut changed = fs::read(&source).unwrap();
    changed.push(0xff);
    fs::write(&source, changed).unwrap();
    let report = compare_descriptor(&descriptor, &index).unwrap();
    assert_eq!(report.local_reusable_bytes, 0);
}

#[test]
fn refuses_to_overwrite_rebuild_output() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    let missing = root.path().join("missing");
    fs::create_dir_all(&sources).unwrap();
    let target = root.path().join("target.bin");
    fs::write(&target, deterministic_bytes(256 * 1024)).unwrap();
    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    stage_missing_chunks(&descriptor, &target, &index, &missing).unwrap();
    let output = root.path().join("output.bin");
    fs::write(&output, b"do not replace").unwrap();
    assert!(rebuild_target(&descriptor, &index, &missing, &output).is_err());
    assert_eq!(fs::read(&output).unwrap(), b"do not replace");
}

#[test]
fn reconstructs_from_chunks_spread_across_multiple_files() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let target_bytes = deterministic_bytes(3 * 1024 * 1024);
    let target = root.path().join("target.bin");
    fs::write(&target, &target_bytes).unwrap();
    let profile = ChunkProfile::named("m").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let split = descriptor.chunks[descriptor.chunks.len() / 2].offset as usize;
    fs::write(sources.join("part-a.bin"), &target_bytes[..split]).unwrap();
    fs::write(sources.join("part-b.bin"), &target_bytes[split..]).unwrap();

    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let report = compare_descriptor(&descriptor, &index).unwrap();
    assert_eq!(report.reuse_ratio, 1.0);
    assert!(
        report
            .plan
            .iter()
            .filter_map(|chunk| chunk.local_source.as_ref())
            .any(|source| source.path.ends_with("part-a.bin"))
    );
    assert!(
        report
            .plan
            .iter()
            .filter_map(|chunk| chunk.local_source.as_ref())
            .any(|source| source.path.ends_with("part-b.bin"))
    );
}

#[test]
fn rejects_descriptor_and_index_profile_mismatch() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let target = root.path().join("target.bin");
    fs::write(&target, deterministic_bytes(512 * 1024)).unwrap();
    let descriptor = create_descriptor(&target, ChunkProfile::named("s").unwrap()).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index
        .index_directory(&sources, ChunkProfile::named("m").unwrap())
        .unwrap();
    assert!(compare_descriptor(&descriptor, &index).is_err());
}

#[test]
fn corrupted_missing_chunk_cannot_be_rebuilt() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    let missing = root.path().join("missing");
    fs::create_dir_all(&sources).unwrap();
    let target = root.path().join("target.bin");
    fs::write(&target, deterministic_bytes(512 * 1024)).unwrap();
    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    stage_missing_chunks(&descriptor, &target, &index, &missing).unwrap();
    let chunk_path = missing.join(format!("{}.chunk", descriptor.chunks[0].hash));
    let mut corrupted = fs::read(&chunk_path).unwrap();
    corrupted[0] ^= 0xff;
    fs::write(&chunk_path, corrupted).unwrap();
    let output = root.path().join("output.bin");
    assert!(rebuild_target(&descriptor, &index, &missing, &output).is_err());
    assert!(!output.exists());
}

#[test]
fn reindex_prunes_files_removed_from_the_authorized_directory() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let source = sources.join("temporary.bin");
    fs::write(&source, deterministic_bytes(256 * 1024)).unwrap();
    let profile = ChunkProfile::named("s").unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    assert_eq!(index.stats().unwrap().files, 1);

    fs::remove_file(source).unwrap();
    index.index_directory(&sources, profile).unwrap();
    assert_eq!(index.stats().unwrap().files, 0);
    assert_eq!(index.stats().unwrap().chunks, 0);
}

#[test]
fn every_named_profile_can_rebuild_exactly() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    let target = root.path().join("target.bin");
    fs::write(&target, deterministic_bytes(1024 * 1024)).unwrap();

    for profile_name in ["s", "m", "l"] {
        let profile = ChunkProfile::named(profile_name).unwrap();
        let descriptor = create_descriptor(&target, profile).unwrap();
        let mut index =
            IndexDb::open(&root.path().join(format!("index-{profile_name}.db"))).unwrap();
        index.index_directory(&sources, profile).unwrap();
        let missing = root.path().join(format!("missing-{profile_name}"));
        stage_missing_chunks(&descriptor, &target, &index, &missing).unwrap();
        let output = root.path().join(format!("rebuilt-{profile_name}.bin"));
        let report = rebuild_target(&descriptor, &index, &missing, &output).unwrap();
        assert!(report.verified);
        assert!(verify_target(&descriptor, &output).unwrap().verified);
    }
}

#[test]
fn fetches_missing_chunks_over_loopback_and_rebuilds() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    let served = root.path().join("served");
    let fetched = root.path().join("fetched");
    fs::create_dir_all(&sources).unwrap();
    let target = root.path().join("target.bin");
    fs::write(&target, deterministic_bytes(2 * 1024 * 1024)).unwrap();
    let profile = ChunkProfile::named("m").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    let staged = stage_missing_chunks(&descriptor, &target, &index, &served).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served_for_thread = served.clone();
    let expected_requests = staged.unique_chunk_files_written;
    let server = thread::spawn(move || {
        serve_chunk_listener(listener, &served_for_thread, Some(expected_requests)).unwrap()
    });

    let fetched_report = fetch_missing_chunks(&descriptor, &index, address, &fetched).unwrap();
    let server_report = server.join().unwrap();
    assert_eq!(fetched_report.chunks_fetched, expected_requests);
    assert_eq!(fetched_report.bytes_fetched, staged.unique_chunk_file_bytes);
    assert_eq!(server_report.chunks_sent, expected_requests);
    assert_eq!(server_report.bytes_sent, staged.unique_chunk_file_bytes);

    let output = root.path().join("network-rebuilt.bin");
    let rebuilt = rebuild_target(&descriptor, &index, &fetched, &output).unwrap();
    assert!(rebuilt.verified);
    assert_eq!(fs::read(output).unwrap(), fs::read(target).unwrap());
}

#[test]
fn network_server_refuses_corrupted_chunk() {
    let root = tempdir().unwrap();
    let sources = root.path().join("sources");
    let served = root.path().join("served");
    let fetched = root.path().join("fetched");
    fs::create_dir_all(&sources).unwrap();
    let target = root.path().join("target.bin");
    fs::write(&target, deterministic_bytes(4096)).unwrap();
    let profile = ChunkProfile::named("s").unwrap();
    let descriptor = create_descriptor(&target, profile).unwrap();
    let mut index = IndexDb::open(&root.path().join("index.db")).unwrap();
    index.index_directory(&sources, profile).unwrap();
    stage_missing_chunks(&descriptor, &target, &index, &served).unwrap();
    let chunk_path = served.join(format!("{}.chunk", descriptor.chunks[0].hash));
    let mut bytes = fs::read(&chunk_path).unwrap();
    bytes[0] ^= 0xff;
    fs::write(&chunk_path, bytes).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served_for_thread = served.clone();
    let server =
        thread::spawn(move || serve_chunk_listener(listener, &served_for_thread, Some(1)).unwrap());
    assert!(fetch_missing_chunks(&descriptor, &index, address, &fetched).is_err());
    let server_report = server.join().unwrap();
    assert_eq!(server_report.unavailable_requests, 1);
    assert_eq!(server_report.chunks_sent, 0);
}

#[test]
fn network_server_defaults_to_loopback_only() {
    let root = tempdir().unwrap();
    let address: SocketAddr = "0.0.0.0:0".parse().unwrap();
    assert!(serve_chunk_directory(address, root.path(), false, Some(1)).is_err());
}
