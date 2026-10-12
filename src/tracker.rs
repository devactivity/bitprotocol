use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};

use crate::bencode::{self, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Started,
    Stopped,
    Completed,
    Interval,
}

impl Event {
    // NOTE: might be errors (lifetime)
    fn as_param(self) -> Option<&'static str> {
        match self {
            Event::Started => Some("started"),
            Event::Stopped => Some("stopped"),
            Event::Completed => Some("completed"),
            Event::Interval => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Announce<'a> {
    pub tracker_url: &'a str,
    pub info_hash: &'a [u8; 20],
    pub peer_id: &'a [u8; 20],

    pub port: u16,
    pub uploaded: u64,
    pub downloaded: u64,

    pub left: u64,
    pub event: Event,
    pub num_peer: u32,
}

impl<'a> Announce<'a> {
    pub fn started(
        tracker_url: &'a str,
        info_hash: &'a [u8; 20],
        peer_id: &'a [u8; 20],
        left: u64,
    ) -> Self {
        Self {
            tracker_url,
            info_hash,
            peer_id,
            port: crate::DEFAULT_PORT,
            uploaded: 0,
            downloaded: 0,
            left,
            event: Event::Started,
            num_peer: 50,
        }
    }

    fn to_url(&self) -> String {
        let separator = if self.tracker_url.contains('?') {
            '&'
        } else {
            '?'
        };

        let mut url = format!(
            "{}{separator}info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&num_peer={}&compact=1",
            self.tracker_url,
            crate::urlencode(self.info_hash),
            crate::urlencode(self.peer_id),
            self.port,
            self.uploaded,
            self.downloaded,
            self.left,
            self.num_peer
        );

        if let Some(event) = self.event.as_param() {
            url.push_str("&event=");
            url.push_str(event);
        };
        url
    }
}

#[derive(Debug, Clone)]
pub struct AnnounceResponse {
    pub interval: Duration,
    pub min_interval: Option<Duration>,

    pub complete: Option<u64>,
    pub incomplete: Option<u64>,

    pub peers: Vec<SocketAddr>,
    pub warning: Option<String>,
}

pub async fn announce(client: &reqwest::Client, req: &Announce<'_>) -> Result<AnnounceResponse> {
    let track = req
        .tracker_url
        .split_once("://")
        .map(|(s, _)| s.to_ascii_lowercase())
        .unwrap_or_default();

    match track.as_str() {
        "http" | "https" => {}
        "udp" => bail!("UDP trackers are not implementeda {}", req.tracker_url),
        other => bail!("unsupported tracker {other:?}: {}", req.tracker_url),
    }

    let url = req.to_url();
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("annoucing to {}", req.tracker_url))?;

    let status = response.status();

    let body = response.bytes().await.context("reading tracker response")?;

    if !status.is_success() {
        bail!(
            "tracker returned HTTP {status}: {}",
            String::from_utf8_lossy(&body[..body.len().min(100)])
        )
    }

    parse_response(&body).with_context(|| format!("parsing response from {}", req.tracker_url))
}

pub fn parse_response(body: &[u8]) -> Result<AnnounceResponse> {
    let root = bencode::decode(body)?;

    if let Some(reason) = root.get("failure reason").and_then(Value::as_str) {
        bail!(
            "
                tracker refused: {reason}
                    "
        );
    }

    let interval = root
        .get("interval")
        .and_then(Value::as_int)
        .filter(|n| *n > 0)
        .unwrap_or(1000) as u64;

    let mut peers = Vec::new();
    if let Some(value) = root.get("peers") {
        parse_peers(value, &mut peers)?;
    }
    if let Some(value) = root.get("peers6") {
        parse_peers6(value, &mut peers)?;
    }

    Ok(AnnounceResponse {
        interval: Duration::from_secs(interval),
        min_interval: root
            .get("min interval")
            .and_then(Value::as_int)
            .filter(|n| *n > 0)
            .map(|n| Duration::from_secs(n as u64)),
        complete: root
            .get("complete")
            .and_then(Value::as_int)
            .map(|n| n as u64),
        incomplete: root
            .get("incomplete")
            .and_then(Value::as_int)
            .map(|n| n as u64),
        peers,
        warning: root
            .get("warning message")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn parse_peers(value: &Value, out: &mut Vec<SocketAddr>) -> Result<()> {
    match value {
        Value::Bytes(raw) => {
            if raw.len() % 6 != 0 {
                // 4 IP + 2 port = 6
                bail!(
                    "compact peer list is {} bytes, not a multiple of 6",
                    raw.len()
                );
            }

            for chunk in raw.chunks_exact(6) {
                let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
                let port = u16::from_be_bytes([chunk[4], chunk[5]]);

                out.push(SocketAddr::V4(SocketAddrV4::new(ip, port)));
            }

            Ok(())
        }
        Value::List(items) => {
            for item in items {
                let ip = item
                    .get("ip")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("peer entry has no ip"))?;

                let port = item
                    .get("port")
                    .and_then(Value::as_int)
                    .ok_or_else(|| anyhow!("peer entry has no port"))?;

                if let Ok(ip) = ip.parse() {
                    out.push(SocketAddr::new(ip, port as u16));
                }
            }

            Ok(())
        }
        _ => bail!("peers is neither a byte string nor a list"),
    }
}

fn parse_peers6(value: &Value, out: &mut Vec<SocketAddr>) -> Result<()> {
    let Some(raw) = value.as_bytes() else {
        bail!("peers6 is not a byte string");
    };

    if raw.len() % 18 != 0 {
        bail!(
            "compact ipv6 peer list is {} bytes, not a multiple of 18",
            raw.len()
        );
    }

    for chunk in raw.chunks_exact(18) {
        let addr: [u8; 16] = chunk[..16].try_into().expect("16 bytes");
        let port = u16::from_be_bytes([chunk[16], chunk[17]]);

        out.push(SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::from(addr),
            port,
            0,
            0,
        )));
    }

    Ok(())
}
