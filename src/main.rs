use bitprotocol::bencode::{self, Value};

const ISO: usize = 60;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: bitprotocol <file.torrent>")?;

    let raw = std::fs::read(&path)?;

    print_value(&bencode::decode(&raw)?, 0);

    let (_, spans) = bencode::decode_dict(&raw)?;

    if let Some(span) = spans.get(&b"info"[..]) {
        println!(
            "\n 'info' dictionary is bytes {}..{} of {} ({} bytes)",
            span.start,
            span.end,
            raw.len(),
            span.len()
        );
    }

    Ok(())
}

fn print_value(value: &Value, indent: usize) {
    let pad = "  ".repeat(indent);

    match value {
        Value::Int(n) => println!("{n}"),
        Value::Bytes(bytes) => println!("{}", describe(bytes)),
        Value::List(items) => {
            println!("list of {}", items.len());
            for item in items {
                print!("{pad}  - ");
                print_value(item, indent + 1);
            }
        }
        Value::Dict(map) => {
            println!("dict of {}", map.len());

            for (key, item) in map {
                print!("{pad}  {}: ", String::from_utf8_lossy(key));
                print_value(item, indent + 1);
            }
        }
    }
}

fn describe(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) if !text.chars().any(char::is_control) => {
            if bytes.len() <= ISO {
                format!("{text:?}")
            } else {
                let cut: String = text.chars().take(ISO).collect();
                format!("{cut:?}... ({} bytes)", bytes.len())
            }
        }
        _ => {
            let head: String = bytes.iter().take(8).map(|b| format!("{b:02x}")).collect();
            format!("<{} bytes of binary> {head}...", bytes.len())
        }
    }
}
