mod bittorrent;
mod bt_peer;
mod bt_seed;
mod bt_tracker;
mod capabilities;
mod chunker;
mod descriptor;
mod hashing;
mod index;
mod magnet;
mod network;
mod profile;
mod workflow;

pub use bittorrent::{
    BtBridgeReport, BtPiecePlan, TorrentV1, TrackerResponse, load_v1_torrent,
    parse_tracker_response, plan_v1_bridge,
};
pub use bt_peer::{BtPeerFetchReport, fetch_v1_from_peer};
pub use bt_seed::{
    BtIndexSeedReport, BtSeedReport, serve_v1_file, serve_v1_file_listener,
    serve_v1_file_listener_until_shutdown, serve_v1_file_until_shutdown, serve_v1_index,
    serve_v1_index_listener, serve_v1_index_until_shutdown,
};
pub use bt_tracker::{
    BtDiscoveryAttempt, BtTrackerAttempt, BtTrackerFetchReport, BtTrackerLifecycleAttempt,
    fetch_v1_via_tracker,
};
pub use capabilities::{CapabilitiesReport, REPORT_FORMAT, REPORT_VERSION, capabilities_report};
pub use chunker::ChunkRecord;
pub use descriptor::{TargetDescriptor, TargetInfo};
pub use index::{IndexDb, IndexReport, IndexStats, SourceLocation};
pub use magnet::{MagnetV1, bind_v1_magnet, parse_v1_magnet};
pub use network::{
    FetchReport, ServeReport, fetch_missing_chunks, serve_chunk_directory, serve_chunk_listener,
};
pub use profile::ChunkProfile;
pub use workflow::{
    CompareReport, RebuildReport, StageMissingReport, VerifyReport, compare_descriptor,
    rebuild_target, stage_missing_chunks, verify_target,
};

pub use descriptor::{create_descriptor, load_descriptor, save_descriptor};
pub use hashing::sha256_file;
