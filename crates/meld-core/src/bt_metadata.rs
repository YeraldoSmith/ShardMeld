use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::bittorrent::{
    parse_extension_handshake, parse_metadata_message_header, parse_v1_info_dictionary,
};
use crate::bt_peer::generate_peer_id;
use crate::{MagnetV1, REPORT_FORMAT, REPORT_VERSION, TorrentV1};

const PROTOCOL_NAME: &[u8; 19] = b"BitTorrent protocol";
const HANDSHAKE_LENGTH: usize = 68;
const EXTENDED_MESSAGE_ID: u8 = 20;
const EXTENDED_HANDSHAKE_ID: u8 = 0;
const LOCAL_UT_METADATA_ID: u8 = 1;
const METADATA_PIECE_LENGTH: usize = 16 * 1024;
const MAX_METADATA_SIZE: usize = 4 * 1024 * 1024;
const MAX_MESSAGE_LENGTH: u32 = 64 * 1024;
const MAX_UNEXPECTED_MESSAGES: usize = 256;
const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BtMetadataFetchReport {
    pub report_format: String,
    pub report_version: u32,
    pub engine_version: String,
    pub peer: SocketAddr,
    pub remote_peer_id_hex: String,
    pub remote_reserved_hex: String,
    pub remote_ut_metadata_id: u8,
    pub metadata_bytes: u64,
    pub metadata_pieces: u64,
    pub info_hash_sha1: String,
    pub info_hash_verified: bool,
    pub torrent_name: String,
}

pub fn fetch_v1_metadata_from_peer(
    magnet: &MagnetV1,
    peer: SocketAddr,
) -> Result<(TorrentV1, BtMetadataFetchReport)> {
    let local_peer_id = generate_peer_id()?;
    let mut stream = TcpStream::connect_timeout(&peer, IO_TIMEOUT)
        .with_context(|| format!("connect to metadata peer {peer}"))?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    send_handshake(&mut stream, &magnet.info_hash_sha1, &local_peer_id)?;
    let (reserved, remote_peer_id) = read_handshake(&mut stream, &magnet.info_hash_sha1)?;
    if reserved[5] & 0x10 == 0 {
        bail!("peer {peer} does not advertise the BEP 10 extension protocol");
    }

    send_extended(
        &mut stream,
        EXTENDED_HANDSHAKE_ID,
        b"d1:md11:ut_metadatai1eee",
    )?;
    let (remote_extension_id, metadata_size) = read_extension_handshake(&mut stream)?;
    let metadata_size = usize::try_from(metadata_size).context("metadata_size is too large")?;
    if metadata_size == 0 || metadata_size > MAX_METADATA_SIZE {
        bail!(
            "peer metadata_size {metadata_size} is outside the supported range 1..={MAX_METADATA_SIZE}"
        );
    }

    let piece_count = metadata_size.div_ceil(METADATA_PIECE_LENGTH);
    let mut metadata = Vec::with_capacity(metadata_size);
    let mut unexpected_messages = 0_usize;
    for piece in 0..piece_count {
        let request = format!("d8:msg_typei0e5:piecei{piece}ee");
        send_extended(&mut stream, remote_extension_id, request.as_bytes())?;
        let expected_length = METADATA_PIECE_LENGTH.min(metadata_size - metadata.len());
        let bytes = read_metadata_piece(
            &mut stream,
            piece,
            metadata_size,
            expected_length,
            remote_extension_id,
            &mut unexpected_messages,
        )?;
        metadata.extend_from_slice(&bytes);
    }

    let actual_info_hash = hex::encode(Sha1::digest(&metadata));
    if actual_info_hash != magnet.info_hash_sha1 {
        bail!(
            "BEP 9 metadata info-hash mismatch: expected {}, got {actual_info_hash}",
            magnet.info_hash_sha1
        );
    }
    let announce = magnet.trackers.first().cloned();
    let announce_list = (!magnet.trackers.is_empty()).then(|| vec![magnet.trackers.clone()]);
    let torrent = parse_v1_info_dictionary(&metadata, announce, announce_list)?;
    let report = BtMetadataFetchReport {
        report_format: REPORT_FORMAT.to_owned(),
        report_version: REPORT_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").to_owned(),
        peer,
        remote_peer_id_hex: hex::encode(remote_peer_id),
        remote_reserved_hex: hex::encode(reserved),
        remote_ut_metadata_id: remote_extension_id,
        metadata_bytes: metadata_size as u64,
        metadata_pieces: piece_count as u64,
        info_hash_sha1: actual_info_hash,
        info_hash_verified: true,
        torrent_name: torrent.name.clone(),
    };
    Ok((torrent, report))
}

fn send_handshake(stream: &mut TcpStream, info_hash_hex: &str, peer_id: &[u8; 20]) -> Result<()> {
    let info_hash = hex::decode(info_hash_hex).context("decode magnet info-hash")?;
    if info_hash.len() != 20 {
        bail!("magnet info-hash must be 20 bytes");
    }
    let mut reserved = [0_u8; 8];
    reserved[5] |= 0x10;
    let mut handshake = Vec::with_capacity(HANDSHAKE_LENGTH);
    handshake.push(PROTOCOL_NAME.len() as u8);
    handshake.extend_from_slice(PROTOCOL_NAME);
    handshake.extend_from_slice(&reserved);
    handshake.extend_from_slice(&info_hash);
    handshake.extend_from_slice(peer_id);
    stream.write_all(&handshake)?;
    stream.flush()?;
    Ok(())
}

fn read_handshake(
    stream: &mut TcpStream,
    expected_info_hash_hex: &str,
) -> Result<([u8; 8], [u8; 20])> {
    let mut handshake = [0_u8; HANDSHAKE_LENGTH];
    stream.read_exact(&mut handshake)?;
    if handshake[0] != PROTOCOL_NAME.len() as u8 || &handshake[1..20] != PROTOCOL_NAME {
        bail!("metadata peer returned an invalid BitTorrent handshake");
    }
    let expected = hex::decode(expected_info_hash_hex)?;
    if handshake[28..48] != expected {
        bail!("metadata peer returned the wrong torrent info-hash");
    }
    let mut reserved = [0_u8; 8];
    reserved.copy_from_slice(&handshake[20..28]);
    let mut peer_id = [0_u8; 20];
    peer_id.copy_from_slice(&handshake[48..68]);
    Ok((reserved, peer_id))
}

fn send_extended(stream: &mut TcpStream, extension_id: u8, payload: &[u8]) -> Result<()> {
    let length = u32::try_from(payload.len() + 2).context("extended message is too large")?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(&[EXTENDED_MESSAGE_ID, extension_id])?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

fn read_extension_handshake(stream: &mut TcpStream) -> Result<(u8, u64)> {
    for _ in 0..64 {
        let Some((message_id, payload)) = read_message(stream)? else {
            continue;
        };
        if message_id != EXTENDED_MESSAGE_ID || payload.first() != Some(&EXTENDED_HANDSHAKE_ID) {
            continue;
        }
        return parse_extension_handshake(&payload[1..]);
    }
    bail!("peer did not send a BEP 10 extension handshake")
}

fn read_metadata_piece(
    stream: &mut TcpStream,
    expected_piece: usize,
    metadata_size: usize,
    expected_length: usize,
    remote_extension_id: u8,
    unexpected_messages: &mut usize,
) -> Result<Vec<u8>> {
    for _ in 0..128 {
        let Some((message_id, payload)) = read_message(stream)? else {
            continue;
        };
        if message_id != EXTENDED_MESSAGE_ID || payload.len() < 2 {
            count_unexpected(unexpected_messages)?;
            continue;
        }
        if payload[0] == EXTENDED_HANDSHAKE_ID {
            let (new_id, new_size) = parse_extension_handshake(&payload[1..])?;
            if new_id != remote_extension_id || new_size != metadata_size as u64 {
                bail!("peer changed its ut_metadata mapping during transfer");
            }
            continue;
        }
        if payload[0] != LOCAL_UT_METADATA_ID {
            count_unexpected(unexpected_messages)?;
            continue;
        }
        let (message_type, piece, total_size, header_length) =
            parse_metadata_message_header(&payload[1..])?;
        if piece != expected_piece as u64 {
            bail!("peer returned metadata piece {piece}, expected {expected_piece}");
        }
        if let Some(total_size) = total_size
            && total_size != metadata_size as u64
        {
            bail!("peer changed metadata total_size during transfer");
        }
        match message_type {
            1 => {
                let data = &payload[1 + header_length..];
                if data.len() != expected_length {
                    bail!(
                        "metadata piece {piece} has {} bytes, expected {expected_length}",
                        data.len()
                    );
                }
                return Ok(data.to_vec());
            }
            2 => bail!("peer rejected metadata piece {piece}"),
            other => bail!("peer returned unsupported ut_metadata msg_type {other}"),
        }
    }
    bail!("peer did not return metadata piece {expected_piece}")
}

fn count_unexpected(count: &mut usize) -> Result<()> {
    *count += 1;
    if *count > MAX_UNEXPECTED_MESSAGES {
        bail!("peer exceeded the unexpected-message limit during metadata exchange");
    }
    Ok(())
}

fn read_message(stream: &mut TcpStream) -> Result<Option<(u8, Vec<u8>)>> {
    let mut length_bytes = [0_u8; 4];
    stream.read_exact(&mut length_bytes)?;
    let length = u32::from_be_bytes(length_bytes);
    if length == 0 {
        return Ok(None);
    }
    if length > MAX_MESSAGE_LENGTH {
        bail!("peer sent oversized BT message of {length} bytes");
    }
    let mut message = vec![0_u8; length as usize];
    stream.read_exact(&mut message)?;
    let message_id = message[0];
    Ok(Some((message_id, message[1..].to_vec())))
}
