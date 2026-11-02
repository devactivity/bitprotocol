use std::time::Duration;

use bitprotocol::{
    hex, human_bytes,
    metainfo::MetaInfo,
    peer::{Bitfield, Message, PeerConn},
    tracker::{self, Announce},
};

use anyhow::{Context, Result, bail};

const PEERS_TO_SHOW: usize = 8;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const LISTEN_WINDOW: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() -> Result<()> {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: bitprotocol <file.torrent>");
        std::process::exit(2);
    };

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

    println!("\n handshaking with up to {PEERS_TO_SHOW} peers ");
    let mut tasks = tokio::task::JoinSet::new();
    for addr in peers.into_iter().take(PEERS_TO_SHOW) {
        let info_hash = torrent.info_hash;
        let piece_count = torrent.piece_count();
        tasks.spawn(async move {
            let result = probe_peer(addr, info_hash, peer_id, piece_count).await;
            (addr, result)
        });
    }

    let mut reached = 0;
    while let Some(joined) = tasks.join_next().await {
        let (addr, result) = joined.context("peer task panicked")?;
        match result {
            Ok(report) => {
                reached += 1;
                println!("\n  {addr} - {report}");
            }
            Err(e) => println!("\n  {addr} - failed: {e}"),
        }
    }

    println!("\nreached {reached} peer(s).");
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

async fn probe_peer(
    addr: std::net::SocketAddr,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    piece_count: usize,
) -> Result<String> {
    let mut conn = PeerConn::connect(addr, info_hash, peer_id, CONNECT_TIMEOUT).await?;

    let mut report = format!(
        "{} (id {})",
        bitprotocol::peer::client_name(&conn.handshake.peer_id),
        hex(&conn.handshake.peer_id[..6]),
    );
    if conn.handshake.extended() {
        report.push_str(" [BEP10]");
    }
    if conn.handshake.dht() {
        report.push_str(" [DHT]");
    }

    conn.write_message(&Message::Interested).await?;

    let deadline = tokio::time::Instant::now() + LISTEN_WINDOW;
    let mut have = Bitfield::empty(piece_count);
    let mut saw_bitfield = false;
    let mut lines = Vec::new();

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match conn.read_message_timeout(remaining).await? {
            Some(Message::Bitfield(bits)) => {
                saw_bitfield = true;
                match Bitfield::from_bytes(bits, piece_count) {
                    Ok(bf) => have = bf,
                    Err(e) => lines.push(format!("bad bitfield: {e}")),
                }
            }
            Some(Message::Have(index)) => {
                saw_bitfield = true;
                have.set_data(index as usize);
            }
            Some(msg) => lines.push(msg.label()),
            None => break,
        }
    }

    if saw_bitfield {
        let pct = 100.0 * have.count() as f64 / piece_count.max(1) as f64;
        lines.insert(
            0,
            format!(
                "has {}/{} pieces ({pct:.1}%){}",
                have.count(),
                piece_count,
                if have.is_complete() { ", seeder" } else { "" }
            ),
        );
    }
    if !lines.is_empty() {
        report.push_str("\n      ");
        report.push_str(&lines.join("\n      "));
    }
    Ok(report)
}
