pub mod bencode;
pub mod metainfo;
pub mod tracker;

pub const CLIENT_PREFIX: &[u8; 8] = b"-BP0001-";

pub const DEFAULT_PORT: u16 = 6881;

pub fn generate_peer_id() -> [u8; 20] {
    let mut id = [0_u8; 20];

    id[..8].copy_from_slice(CLIENT_PREFIX);

    for byte in &mut id[8..] {
        *byte = b'0' + rand::random::<u8>() % 10;
    }

    id
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn urlencode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 5);

    for &b in bytes {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-'..=b'_' | b'.'..=b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("{b:02X}")),
        }
    }

    out
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];

    let mut value = bytes as f64;
    let mut unit = 0;

    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}
