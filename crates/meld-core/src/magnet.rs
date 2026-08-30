use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::TorrentV1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MagnetV1 {
    pub info_hash_sha1: String,
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
}

pub fn parse_v1_magnet(value: &str) -> Result<MagnetV1> {
    let url = Url::parse(value).context("parse magnet URI")?;
    if url.scheme() != "magnet" {
        bail!("expected a magnet URI");
    }
    if url.fragment().is_some() {
        bail!("magnet URI fragments are not supported");
    }

    let mut info_hash = None;
    let mut display_name = None;
    let mut trackers = Vec::new();
    let mut seen_trackers = HashSet::new();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "xt" => {
                let Some(encoded) = value.strip_prefix("urn:btih:") else {
                    continue;
                };
                let decoded = decode_btih(encoded)?;
                if info_hash
                    .as_ref()
                    .is_some_and(|current| current != &decoded)
                {
                    bail!("magnet URI contains conflicting v1 info hashes");
                }
                info_hash = Some(decoded);
            }
            "dn" if display_name.is_none() => display_name = Some(value.into_owned()),
            "tr" => {
                let tracker = value.into_owned();
                validate_tracker(&tracker)?;
                if seen_trackers.insert(tracker.clone()) {
                    trackers.push(tracker);
                }
            }
            _ => {}
        }
    }

    Ok(MagnetV1 {
        info_hash_sha1: info_hash.context("magnet URI has no v1 urn:btih exact topic")?,
        display_name,
        trackers,
    })
}

pub fn bind_v1_magnet(magnet: &MagnetV1, torrent: &TorrentV1) -> Result<TorrentV1> {
    if magnet.info_hash_sha1 != torrent.info_hash_sha1 {
        bail!(
            "magnet/torrent info-hash mismatch: magnet has {}, metadata has {}",
            magnet.info_hash_sha1,
            torrent.info_hash_sha1
        );
    }
    let mut bound = torrent.clone();
    if !magnet.trackers.is_empty() {
        bound.announce = magnet.trackers.first().cloned();
        bound.announce_list = Some(vec![magnet.trackers.clone()]);
    }
    Ok(bound)
}

fn validate_tracker(value: &str) -> Result<()> {
    let tracker = Url::parse(value).context("parse magnet tracker URL")?;
    if !matches!(tracker.scheme(), "http" | "https" | "udp") {
        bail!(
            "magnet tracker uses unsupported scheme {}",
            tracker.scheme()
        );
    }
    if tracker.host_str().is_none() {
        bail!("magnet tracker URL has no host");
    }
    Ok(())
}

fn decode_btih(value: &str) -> Result<String> {
    if value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(value.to_ascii_lowercase());
    }
    if value.len() != 32 {
        bail!("v1 magnet info hash must be 40 hexadecimal or 32 base32 characters");
    }
    let mut output = Vec::with_capacity(20);
    let mut accumulator = 0_u32;
    let mut bits = 0_u32;
    for byte in value.bytes() {
        let value = match byte.to_ascii_uppercase() {
            b'A'..=b'Z' => byte.to_ascii_uppercase() - b'A',
            b'2'..=b'7' => byte - b'2' + 26,
            _ => bail!("v1 magnet info hash contains an invalid base32 character"),
        };
        accumulator = (accumulator << 5) | u32::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
            accumulator &= (1_u32 << bits).saturating_sub(1);
        }
    }
    if output.len() != 20 || bits != 0 {
        bail!("v1 magnet base32 info hash has an invalid length");
    }
    Ok(hex::encode(output))
}

#[cfg(test)]
mod tests {
    use super::parse_v1_magnet;

    #[test]
    fn parses_hex_and_base32_btih_with_deduplicated_trackers() {
        let hex = parse_v1_magnet(
            "magnet:?xt=urn:btih:CBFE49F2C4D44A6A4823EBFA8C829351755D90BB&dn=sqlite3.c&tr=http%3A%2F%2F127.0.0.1%3A45995%2Fannounce&tr=http%3A%2F%2F127.0.0.1%3A45995%2Fannounce",
        )
        .unwrap();
        let base32 =
            parse_v1_magnet("magnet:?xt=urn:btih:ZP7ET4WE2RFGUSBD5P5IZAUTKF2V3EF3").unwrap();
        assert_eq!(hex.info_hash_sha1, base32.info_hash_sha1);
        assert_eq!(hex.display_name.as_deref(), Some("sqlite3.c"));
        assert_eq!(hex.trackers.len(), 1);
    }

    #[test]
    fn rejects_conflicting_info_hashes_and_unsupported_tracker_schemes() {
        assert!(
            parse_v1_magnet(
                "magnet:?xt=urn:btih:CBFE49F2C4D44A6A4823EBFA8C829351755D90BB&xt=urn:btih:0000000000000000000000000000000000000000"
            )
            .is_err()
        );
        assert!(
            parse_v1_magnet(
                "magnet:?xt=urn:btih:CBFE49F2C4D44A6A4823EBFA8C829351755D90BB&tr=ftp%3A%2F%2Ftracker.example%2Fannounce"
            )
            .is_err()
        );
    }
}
