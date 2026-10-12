use bitprotocol::{human_bytes, metainfo::MetaInfo};

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: bitprotocol <file.torrent>")?;

    let torrent = MetaInfo::from_file(&path)?;

    print_value(&torrent);

    println!(
        "\n piece 0 hash {}",
        bitprotocol::hex(&torrent.piece_hashes[0])
    );
    println!("  magnet    {}", torrent.magnet());

    Ok(())
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
