use std::time::Duration;

use bitprotocol::{
    human_bytes,
    metainfo::MetaInfo,
    tracker::{self, Announce},
};

use anyhow::{Context, Result, bail};

const PEERS_TO_SHOW: usize = 12;

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: bitprotocol <file.torrent>")?;

    let torrent = MetaInfo::from_file(&path)?;
    print_value(&torrent);

    let peer_id = bitprotocol::generate_peer_id();
    println!("\nmy peer id: {}", String::from_utf8_lossy(&peer_id));

    let peers = find_peers(&torrent, &peer_id).await?;
    if peers.is_empty() {
        bail!(
            "
        no peers returned
            "
        );
    }

    println!("\n peers ");
    for addr in peers.iter().take(PEERS_TO_SHOW) {
        println!("  {addr}");
    }

    if peers.len() > PEERS_TO_SHOW {
        println!("  ..... and {} more", peers.len() - PEERS_TO_SHOW);
    }

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

async fn find_peers(torrent: &MetaInfo, peer_id: &[u8; 20]) -> Result<Vec<std::net::SocketAddr>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("bitprotocol/0.1")
        .build()?;

    println!("\n announcing ");

    for url in torrent.trackers() {
        let request = Announce::started(url, &torrent.info_hash, peer_id, torrent.total_length);

        match tracker::announce(&client, &request).await {
            Ok(response) => {
                println!(
                    "{url}\n   {} peers, {} seeders, {} leechers, re-announce in {}s",
                    response.peers.len(),
                    response.complete.map_or("?".into(), |n| n.to_string()),
                    response.incomplete.map_or("?".into(), |n| n.to_string()),
                    response.interval.as_secs()
                );

                if let Some(warning) = &response.warning {
                    println!("   warning: {warning}");
                }

                if !response.peers.is_empty() {
                    return Ok(response.peers);
                }
            }

            Err(e) => println!("  {url}\n {e}"),
        }
    }

    Ok(Vec::new())
}
