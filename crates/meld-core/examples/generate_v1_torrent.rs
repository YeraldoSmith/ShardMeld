use std::fs::OpenOptions;
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use sha1::{Digest, Sha1};

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let target = PathBuf::from(arguments.next().context("missing target file")?);
    let output = PathBuf::from(arguments.next().context("missing output .torrent")?);
    let piece_length: usize = arguments
        .next()
        .context("missing piece length")?
        .to_str()
        .context("piece length is not UTF-8")?
        .parse()
        .context("invalid piece length")?;
    let trackers = arguments
        .map(|value| {
            value
                .to_str()
                .context("tracker URL is not UTF-8")
                .map(str::to_owned)
        })
        .collect::<Result<Vec<_>>>()?;
    let announce = trackers.first().cloned();
    if piece_length == 0 {
        bail!("piece length must be greater than zero");
    }

    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .context("target filename is not UTF-8")?;
    let total_length = std::fs::metadata(&target)
        .with_context(|| format!("read metadata for {}", target.display()))?
        .len();
    let mut reader = std::fs::File::open(&target)
        .with_context(|| format!("open target {}", target.display()))?;
    let mut buffer = vec![0_u8; piece_length];
    let mut piece_hashes = Vec::new();
    loop {
        let mut filled = 0;
        while filled < buffer.len() {
            let read = reader.read(&mut buffer[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled == 0 {
            break;
        }
        piece_hashes.extend_from_slice(&Sha1::digest(&buffer[..filled]));
    }

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .with_context(|| format!("create torrent {}", output.display()))?;
    let mut writer = BufWriter::new(file);
    writer.write_all(b"d")?;
    if let Some(announce) = &announce {
        writer.write_all(b"8:announce")?;
        write!(writer, "{}:", announce.len())?;
        writer.write_all(announce.as_bytes())?;
    }
    if trackers.len() > 1 {
        writer.write_all(b"13:announce-listl")?;
        for tracker in &trackers {
            writer.write_all(b"l")?;
            write!(writer, "{}:", tracker.len())?;
            writer.write_all(tracker.as_bytes())?;
            writer.write_all(b"e")?;
        }
        writer.write_all(b"e")?;
    }
    writer.write_all(b"4:info")?;
    writer.write_all(b"d6:lengthi")?;
    write!(writer, "{total_length}")?;
    writer.write_all(b"e4:name")?;
    write!(writer, "{}:", name.len())?;
    writer.write_all(name.as_bytes())?;
    writer.write_all(b"12:piece lengthi")?;
    write!(writer, "{piece_length}")?;
    writer.write_all(b"e6:pieces")?;
    write!(writer, "{}:", piece_hashes.len())?;
    writer.write_all(&piece_hashes)?;
    writer.write_all(b"ee")?;
    writer.flush()?;

    println!(
        "torrent={} target_bytes={} piece_length={} pieces={}",
        output.display(),
        total_length,
        piece_length,
        piece_hashes.len() / 20
    );
    Ok(())
}
