use std::fs::OpenOptions;
use std::io::BufWriter;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use meld_core::{
    ChunkProfile, IndexDb, bind_v1_magnet, capabilities_report, compare_descriptor,
    create_descriptor, fetch_missing_chunks, fetch_v1_from_peer, fetch_v1_via_tracker,
    load_descriptor, load_v1_torrent, parse_v1_magnet, plan_v1_bridge, rebuild_target,
    save_descriptor, serve_chunk_directory, serve_v1_file, serve_v1_index, stage_missing_chunks,
    verify_target,
};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(
    name = "shardmeld",
    version,
    about = "Reconstruct first. Transfer only what's missing."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Report implemented features, deferred features, and stable format versions.
    Capabilities {
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Index files from one explicitly authorized directory.
    Index {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value = "m")]
        profile: String,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Build a deterministic .meld descriptor from a target file.
    Describe {
        #[arg(long)]
        target: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value = "m")]
        profile: String,
    },
    /// Compare a descriptor against a local index.
    Compare {
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Map local CDC reuse onto standard v1 single-file BitTorrent pieces.
    BtPlan {
        #[arg(long)]
        torrent: PathBuf,
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Fetch only missing 16 KiB blocks from one standard BitTorrent v1 peer.
    BtFetchPeer {
        #[arg(long)]
        torrent: PathBuf,
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        peer: SocketAddr,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Discover standard v1 peers through HTTP(S)/UDP trackers, then fetch missing blocks.
    BtFetchTracker {
        #[arg(long)]
        torrent: PathBuf,
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        db: PathBuf,
        /// Override the announce URL embedded in the .torrent file.
        #[arg(long)]
        tracker: Option<String>,
        /// Port reported to trackers. Use the same port as a separately running seed command.
        #[arg(long, default_value_t = 6881)]
        announce_port: u16,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Bind a v1 magnet to trusted local .torrent metadata, then discover and fetch.
    BtFetchMagnet {
        #[arg(long)]
        magnet: String,
        /// Local v1 .torrent metadata whose info hash must match the magnet.
        #[arg(long)]
        metadata: PathBuf,
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        db: PathBuf,
        /// Override both magnet and metadata tracker URLs.
        #[arg(long)]
        tracker: Option<String>,
        #[arg(long, default_value_t = 6881)]
        announce_port: u16,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Seed a fully verified file to standard BitTorrent v1 peers.
    BtSeedFile {
        #[arg(long)]
        torrent: PathBuf,
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, default_value = "127.0.0.1:45996")]
        bind: SocketAddr,
        #[arg(long)]
        allow_non_loopback: bool,
        #[arg(long)]
        max_connections: Option<u64>,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Seed verified BitTorrent Pieces reconstructed on demand from the local CDC index.
    BtSeedIndex {
        #[arg(long)]
        torrent: PathBuf,
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value = "127.0.0.1:45996")]
        bind: SocketAddr,
        #[arg(long)]
        allow_non_loopback: bool,
        #[arg(long)]
        max_connections: Option<u64>,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Test helper: export only locally missing chunks from the original target.
    StageMissing {
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        target: PathBuf,
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        out_dir: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Serve a staged chunk directory over the minimal ShardMeld v0.2 protocol.
    ServeChunks {
        #[arg(long)]
        source: PathBuf,
        #[arg(long, default_value = "127.0.0.1:45872")]
        bind: SocketAddr,
        #[arg(long)]
        allow_non_loopback: bool,
        #[arg(long)]
        max_requests: Option<u64>,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Fetch only locally missing chunks from a ShardMeld v0.2 peer.
    FetchMissing {
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        peer: SocketAddr,
        #[arg(long)]
        out_dir: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Rebuild a target from indexed local chunks and a missing-chunk directory.
    Rebuild {
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        missing_source: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Verify a file against a descriptor's size and SHA-256.
    Verify {
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Capabilities { json } => {
            let report = capabilities_report();
            save_report(&report, json.as_deref())?;
            println!(
                "engine_version={} report_format={} report_version={} implemented={} deferred={}",
                report.engine_version,
                report.report_format,
                report.report_version,
                report.implemented.len(),
                report.deferred.len()
            );
        }
        Command::Index {
            source,
            db,
            profile,
            json,
        } => {
            let profile = ChunkProfile::named(&profile)?;
            let mut index = IndexDb::open(&db)?;
            let report = index.index_directory(&source, profile)?;
            save_report(&report, json.as_deref())?;
            println!(
                "indexed_files={} indexed_bytes={} chunks={} database_bytes={} elapsed_ms={} skipped={}",
                report.files_indexed,
                report.bytes_indexed,
                report.chunks_indexed,
                report.database_bytes,
                report.elapsed_ms,
                report.skipped_entries.len()
            );
        }
        Command::Describe {
            target,
            out,
            profile,
        } => {
            let descriptor = create_descriptor(&target, ChunkProfile::named(&profile)?)?;
            save_descriptor(&descriptor, &out)?;
            println!(
                "descriptor={} target_bytes={} chunks={} sha256={}",
                out.display(),
                descriptor.target.size,
                descriptor.chunks.len(),
                descriptor.target.sha256
            );
        }
        Command::Compare {
            descriptor,
            db,
            json,
        } => {
            let descriptor = load_descriptor(&descriptor)?;
            let index = IndexDb::open(&db)?;
            let report = compare_descriptor(&descriptor, &index)?;
            save_report(&report, json.as_deref())?;
            println!(
                "target_bytes={} local_reusable_bytes={} missing_payload_bytes={} reuse_ratio={:.4}% matched_chunks={}/{}",
                report.target_bytes,
                report.local_reusable_bytes,
                report.missing_payload_bytes,
                report.reuse_ratio * 100.0,
                report.matched_chunks,
                report.target_chunks
            );
        }
        Command::BtPlan {
            torrent,
            descriptor,
            db,
            json,
        } => {
            let torrent = load_v1_torrent(&torrent)?;
            let descriptor = load_descriptor(&descriptor)?;
            let index = IndexDb::open(&db)?;
            let report = plan_v1_bridge(&torrent, &descriptor, &index)?;
            save_report(&report, json.as_deref())?;
            println!(
                "info_hash={} pieces_fully_local={}/{} local_coverage={:.4}% fully_reconstructable_piece_bytes={} missing_bytes={}",
                report.info_hash_sha1,
                report.fully_local_pieces,
                report.total_pieces,
                report.local_coverage_ratio * 100.0,
                report.fully_reconstructable_piece_bytes,
                report.missing_bytes
            );
        }
        Command::BtFetchPeer {
            torrent,
            descriptor,
            db,
            peer,
            out,
            json,
        } => {
            let torrent = load_v1_torrent(&torrent)?;
            let descriptor = load_descriptor(&descriptor)?;
            let index = IndexDb::open(&db)?;
            let report = fetch_v1_from_peer(&torrent, &descriptor, &index, peer, &out)?;
            save_report(&report, json.as_deref())?;
            println!(
                "peer={} info_hash={} peers_connected={} contributors={} reassigned_pieces={} local_bytes={} genuinely_missing={} network_payload={} redundant={} blocks={} sha256={} verified={}",
                report.peer,
                report.info_hash_sha1,
                report.peers_connected,
                report.contributing_peers.len(),
                report.pieces_reassigned,
                report.local_bytes_available,
                report.genuinely_missing_bytes,
                report.network_payload_bytes,
                report.network_redundant_bytes,
                report.network_block_requests,
                report.output_sha256,
                report.verified
            );
        }
        Command::BtFetchTracker {
            torrent,
            descriptor,
            db,
            tracker,
            announce_port,
            out,
            json,
        } => {
            let torrent = load_v1_torrent(&torrent)?;
            let descriptor = load_descriptor(&descriptor)?;
            let index = IndexDb::open(&db)?;
            let report = fetch_v1_via_tracker(
                &torrent,
                &descriptor,
                &index,
                tracker.as_deref(),
                announce_port,
                &out,
            )?;
            save_report(&report, json.as_deref())?;
            println!(
                "tracker={} peers_discovered={} peers_attempted={} peers_connected={} contributors={} reassigned_pieces={} selected_peer={} local_bytes={} genuinely_missing={} network_payload={} sha256={} verified={}",
                report.tracker,
                report.peers_discovered,
                report.peers_attempted.len(),
                report.transfer.peers_connected,
                report.transfer.contributing_peers.len(),
                report.transfer.pieces_reassigned,
                report.selected_peer,
                report.transfer.local_bytes_available,
                report.transfer.genuinely_missing_bytes,
                report.transfer.network_payload_bytes,
                report.transfer.output_sha256,
                report.verified
            );
        }
        Command::BtFetchMagnet {
            magnet,
            metadata,
            descriptor,
            db,
            tracker,
            announce_port,
            out,
            json,
        } => {
            let magnet = parse_v1_magnet(&magnet)?;
            let metadata = load_v1_torrent(&metadata)?;
            let torrent = bind_v1_magnet(&magnet, &metadata)?;
            let descriptor = load_descriptor(&descriptor)?;
            let index = IndexDb::open(&db)?;
            let report = fetch_v1_via_tracker(
                &torrent,
                &descriptor,
                &index,
                tracker.as_deref(),
                announce_port,
                &out,
            )?;
            save_report(&report, json.as_deref())?;
            println!(
                "magnet_info_hash={} metadata={} peers_connected={} contributors={} network_payload={} sha256={} verified={}",
                magnet.info_hash_sha1,
                torrent.name,
                report.transfer.peers_connected,
                report.transfer.contributing_peers.len(),
                report.transfer.network_payload_bytes,
                report.transfer.output_sha256,
                report.verified
            );
        }
        Command::BtSeedFile {
            torrent,
            descriptor,
            file,
            bind,
            allow_non_loopback,
            max_connections,
            json,
        } => {
            let torrent = load_v1_torrent(&torrent)?;
            let descriptor = load_descriptor(&descriptor)?;
            println!(
                "starting_bt_seed bind={} file={} info_hash={} max_connections={}",
                bind,
                file.display(),
                torrent.info_hash_sha1,
                max_connections
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unlimited".to_owned())
            );
            let report = serve_v1_file(
                &torrent,
                &descriptor,
                &file,
                bind,
                allow_non_loopback,
                max_connections,
            )?;
            save_report(&report, json.as_deref())?;
            println!(
                "seed_stopped bind={} connections={} handshakes={} requests={} payload={} cancels={} errors={} verified={}",
                report.bind,
                report.connections,
                report.successful_handshakes,
                report.block_requests,
                report.payload_bytes_sent,
                report.cancel_messages_received,
                report.protocol_errors,
                report.source_verified
            );
        }
        Command::BtSeedIndex {
            torrent,
            descriptor,
            db,
            bind,
            allow_non_loopback,
            max_connections,
            json,
        } => {
            let torrent = load_v1_torrent(&torrent)?;
            let descriptor = load_descriptor(&descriptor)?;
            let index = IndexDb::open(&db)?;
            println!(
                "starting_bt_index_seed bind={} db={} info_hash={} max_connections={}",
                bind,
                db.display(),
                torrent.info_hash_sha1,
                max_connections
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unlimited".to_owned())
            );
            let report = serve_v1_index(
                &torrent,
                &descriptor,
                &index,
                &db,
                bind,
                allow_non_loopback,
                max_connections,
            )?;
            save_report(&report, json.as_deref())?;
            println!(
                "index_seed_stopped bind={} advertised={}/{} connections={} handshakes={} requests={} payload={} local_chunks={} local_bytes={} errors={}",
                report.bind,
                report.advertised_pieces,
                report.total_pieces,
                report.connections,
                report.successful_handshakes,
                report.block_requests,
                report.payload_bytes_sent,
                report.on_demand_local_chunks_read,
                report.on_demand_local_bytes_read,
                report.protocol_errors
            );
        }
        Command::StageMissing {
            descriptor,
            target,
            db,
            out_dir,
            json,
        } => {
            let descriptor = load_descriptor(&descriptor)?;
            let index = IndexDb::open(&db)?;
            let report = stage_missing_chunks(&descriptor, &target, &index, &out_dir)?;
            save_report(&report, json.as_deref())?;
            println!(
                "missing_payload_bytes={} chunk_occurrences={} unique_chunk_files={}",
                report.missing_payload_bytes,
                report.missing_chunk_occurrences,
                report.unique_chunk_files_written
            );
        }
        Command::ServeChunks {
            source,
            bind,
            allow_non_loopback,
            max_requests,
            json,
        } => {
            println!(
                "starting_chunk_server bind={} source={} max_requests={}",
                bind,
                source.display(),
                max_requests
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unlimited".to_owned())
            );
            let report = serve_chunk_directory(bind, &source, allow_non_loopback, max_requests)?;
            save_report(&report, json.as_deref())?;
            println!(
                "server_stopped bind={} connections={} requests={} chunks_sent={} bytes_sent={} unavailable={} connection_errors={}",
                report.bound_address,
                report.connections,
                report.requests_received,
                report.chunks_sent,
                report.bytes_sent,
                report.unavailable_requests,
                report.connection_errors
            );
        }
        Command::FetchMissing {
            descriptor,
            db,
            peer,
            out_dir,
            json,
        } => {
            let descriptor = load_descriptor(&descriptor)?;
            let index = IndexDb::open(&db)?;
            let report = fetch_missing_chunks(&descriptor, &index, peer, &out_dir)?;
            save_report(&report, json.as_deref())?;
            println!(
                "peer={} missing_occurrences={} unique_needed={} fetched_chunks={} fetched_bytes={} existing_verified={}",
                report.peer,
                report.missing_chunk_occurrences,
                report.unique_chunks_needed,
                report.chunks_fetched,
                report.bytes_fetched,
                report.existing_verified_chunks
            );
        }
        Command::Rebuild {
            descriptor,
            db,
            missing_source,
            out,
            json,
        } => {
            let descriptor = load_descriptor(&descriptor)?;
            let index = IndexDb::open(&db)?;
            let report = rebuild_target(&descriptor, &index, &missing_source, &out)?;
            save_report(&report, json.as_deref())?;
            println!(
                "output={} target_bytes={} local_bytes={} missing_source_bytes={} sha256={} verified={}",
                report.output.display(),
                report.target_bytes,
                report.local_bytes_read,
                report.missing_source_bytes_read,
                report.output_sha256,
                report.verified
            );
        }
        Command::Verify {
            descriptor,
            file,
            json,
        } => {
            let descriptor = load_descriptor(&descriptor)?;
            let report = verify_target(&descriptor, &file)?;
            save_report(&report, json.as_deref())?;
            println!(
                "file={} bytes={}/{} sha256={} verified={}",
                report.file.display(),
                report.actual_bytes,
                report.expected_bytes,
                report.actual_sha256,
                report.verified
            );
            if !report.verified {
                bail!("verification failed");
            }
        }
    }
    Ok(())
}

fn save_report<T: Serialize>(report: &T, json_path: Option<&Path>) -> Result<()> {
    if let Some(path) = json_path {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("create JSON report {}", path.display()))?;
        serde_json::to_writer_pretty(BufWriter::new(file), report)
            .with_context(|| format!("write JSON report {}", path.display()))?;
    }
    Ok(())
}
