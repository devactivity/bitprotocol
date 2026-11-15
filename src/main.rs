use std::io::Write;
use std::ops::Range;

use bitprotocol::{human_bytes, metainfo::MetaInfo, storage::Storage};

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .context("usage: bitprotocol <file.torrent> [DIR]")?;
    let dir = args.next().unwrap_or_else(|| "downloads".to_string());

    let torrent = MetaInfo::from_file(&path)?;
    print_value(&torrent);

    println!("\n checking {dir} ");
    let storage = Storage::create(&dir, &torrent).with_context(|| format!("preparing {dir}"))?;

    let data = storage.resume_download(&torrent, |done, total| {
        if done % 64 == 0 || done == total {
            print!("\r hashed {done}/{total} pieces");
            let _ = std::io::stdout().flush();
        }
    })?;
    println!();

    let present: u64 = (0..torrent.piece_count())
        .filter(|i| data.has_data(*i))
        .map(|i| torrent.piece_size(i))
        .sum();

    println!(
        "  {}/{} pieces present ({} of {})",
        data.count(),
        torrent.piece_count(),
        human_bytes(present),
        human_bytes(torrent.total_length)
    );

    println!("\n files ");
    for file in &torrent.files {
        let pieces = data_overlap(&torrent, file.offset, file.length);
        let total = pieces.len();
        let data = pieces.filter(|i| data.has_data(*i)).count();
        let status = match data {
            _ if data == total => "complete",
            0 => "missing",
            _ => "partial",
        };

        println!(
            "  {status:<9} {data:>5}/{total:<5} pieces {}",
            file.path.display()
        );
    }

    Ok(())
}

fn data_overlap(torrent: &MetaInfo, offset: u64, length: u64) -> Range<usize> {
    if length == 0 {
        return 0..0;
    }

    let first = offset / torrent.piece_length;
    let last = (offset + length - 1) / torrent.piece_length;

    first as usize..last as usize + 1
}

fn print_value(m: &MetaInfo) {
    println!("== torrent ==");
    println!("  name       {}", m.name);
    println!("  info hash  {}", m.info_hash_hex());
    println!(
        "
      size    {} ({} bytes)
        ",
        human_bytes(m.total_length),
        m.total_length
    );

    println!(
        "  pieces   {} {} (last piece {})",
        m.piece_count(),
        human_bytes(m.piece_length),
        human_bytes(m.piece_size(m.piece_count() - 1))
    );

    println!("  private {}", m.private);

    println!("  trackers");
    for url in m.trackers() {
        println!("    {url}");
    }

    if !m.is_single_file() {
        println!(" files ({})", m.files.len());

        for f in m.files.iter().take(10) {
            println!("   {:>12} {}", human_bytes(f.length), f.path.display());
        }

        if m.files.len() > 10 {
            println!("    ... and {} more", m.files.len() - 10);
        }
    }
}
