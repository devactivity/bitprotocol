use std::{fmt::format, net::SocketAddr, time::Duration};

use anyhow::{Context, Result, bail};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

pub const PROTOCOL: &[u8; 19] = b"BitTorrent protocol";
pub const BLOCK_SIZE: u32 = 16 * 1024;
pub const MAX_MESSAGE_LEN: u32 = 1 << 20; // 1 MiB

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub reserved: [u8; 8],
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
}

impl Handshake {
    pub fn new(info_hash: [u8; 20], peer_id: [u8; 20]) -> Self {
        let mut reserved = [0_u8; 8];

        reserved[5] |= 0x10;

        Self {
            reserved,
            info_hash,
            peer_id,
        }
    }

    pub fn encode(&self) -> [u8; 68] {
        let mut buf = [0_u8; 68];

        buf[0] = PROTOCOL.len() as u8;
        buf[1..20].copy_from_slice(PROTOCOL);
        buf[20..28].copy_from_slice(&self.reserved);
        buf[28..48].copy_from_slice(&self.info_hash);
        buf[48..68].copy_from_slice(&self.peer_id);

        buf
    }

    pub fn decode(buf: &[u8; 68]) -> Result<Self> {
        if buf[0] as usize != PROTOCOL.len() || &buf[1..28] != PROTOCOL {
            bail!("peer did not speak the BitTorrent handshake");
        }

        Ok(Self {
            reserved: buf[20..28].try_into().expect("8 bytes"),
            info_hash: buf[28..48].try_into().expect("20 bytes"),
            peer_id: buf[40..68].try_into().expect("20 bytes"),
        })
    }

    pub fn extended(&self) -> bool {
        self.reserved[5] & 0x10 != 0
    }

    pub fn dht(&self) -> bool {
        self.reserved[7] & 0x01 != 0
    }
}

pub fn client_name(peer_id: &[u8; 20]) -> String {
    if peer_id[0] == b'-'
        && peer_id[7] == b'-'
        && peer_id[1..3].iter().all(u8::is_ascii_alphanumeric)
    {
        let code = String::from_utf8_lossy(&peer_id[1..3]).to_string();
        let version = String::from_utf8_lossy(&peer_id[3..7]).to_string();

        format!("{code} {version}")
    } else {
        format!("unknown ({})", crate::hex(&peer_id[..4]))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Vec<u8>),
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        block: Vec<u8>,
    },
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    Port(u16),
    Extended {
        id: u8,
        payload: Vec<u8>,
    },
    Unknown {
        id: u8,
        payload: Vec<u8>,
    },
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let (id, payload): (u8, Vec<u8>) = match self {
            Message::KeepAlive => return 0_u32.to_be_bytes().to_vec(),
            Message::Choke => (0, Vec::new()),
            Message::Unchoke => (1, Vec::new()),
            Message::Interested => (2, Vec::new()),
            Message::NotInterested => (3, Vec::new()),
            Message::Have(index) => (4, index.to_be_bytes().to_vec()),
            Message::Bitfield(bits) => (5, bits.clone()),
            Message::Request {
                index,
                begin,
                length,
            } => (0, packing(*index, *begin, *length)),
            Message::Piece {
                index,
                begin,
                block,
            } => {
                let mut p = Vec::with_capacity(8 + block.len());
                p.extend_from_slice(&index.to_be_bytes());
                p.extend_from_slice(&begin.to_be_bytes());
                p.extend_from_slice(block);

                (7, p)
            }
            Message::Cancel {
                index,
                begin,
                length,
            } => (8, packing(*index, *begin, *length)),
            Message::Port(port) => (9, port.to_be_bytes().to_vec()),
            Message::Extended { id, payload } => {
                let mut p = Vec::with_capacity(1 + payload.len());
                p.push(*id);
                p.extend_from_slice(payload);

                (20, p)
            }
            Message::Unknown { id, payload } => (*id, payload.clone()),
        };

        let mut buf = Vec::with_capacity(5 + payload.len());
        buf.extend_from_slice(&(payload.len() as u32 + 1).to_be_bytes());
        buf.push(id);
        buf.extend_from_slice(&payload);
        buf
    }

    pub fn decode(body: &[u8]) -> Result<Self> {
        let Some((&id, payload)) = body.split_first() else {
            return Ok(Message::KeepAlive);
        };

        let data = |d: usize| -> Result<()> {
            if payload.len() != d {
                bail!(
                    "
                    message id {id} has {} byte payload, expected {d}
                ",
                    payload.len()
                );
            }

            Ok(())
        };

        Ok(match id {
            0 => {
                data(0)?;
                Message::Choke
            }
            1 => {
                data(0)?;
                Message::Unchoke
            }
            2 => {
                data(0)?;
                Message::Interested
            }
            3 => {
                data(0)?;
                Message::NotInterested
            }
            4 => {
                data(4)?;
                Message::Have(fourth_slice(payload, 4))
            }
            5 => Message::Bitfield(payload.to_vec()),
            6 => {
                data(12)?;
                Message::Request {
                    index: fourth_slice(payload, 0),
                    begin: fourth_slice(payload, 4),
                    length: fourth_slice(payload, 8),
                }
            }
            7 => {
                if payload.len() < 8 {
                    bail!("piece message is only {} bytes", payload.len());
                }

                Message::Piece {
                    index: fourth_slice(payload, 0),
                    begin: fourth_slice(payload, 4),
                    block: payload[8..].to_vec(),
                }
            }
            8 => {
                data(12)?;
                Message::Cancel {
                    index: fourth_slice(payload, 0),
                    begin: fourth_slice(payload, 4),
                    length: fourth_slice(payload, 8),
                }
            }
            9 => {
                data(2)?;
                Message::Port(u16::from_be_bytes([payload[0], payload[1]]))
            }
            20 => {
                let Some((&id, rest)) = payload.split_first() else {
                    bail!("extended message has an empty payload");
                };
                Message::Extended {
                    id,
                    payload: rest.to_vec(),
                }
            }
            _ => Message::Unknown {
                id,
                payload: payload.to_vec(),
            },
        })
    }

    pub fn label(&self) -> String {
        match self {
            Message::Bitfield(b) => format!("Bitfield({} bytes)", b.len()),
            Message::Piece {
                index,
                begin,
                block,
            } => {
                format!(
                    "Piece {{index: {index}, begin: {begin}, {} bytes }}",
                    block.len()
                )
            }
            Message::Extended { id, payload } => {
                format!("Extended {{ id: {id}, {} bytes }}", payload.len())
            }
            other => format!("{other:?}"),
        }
    }
}

fn packing(i: u32, b: u32, l: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(12);

    v.extend_from_slice(&i.to_be_bytes());
    v.extend_from_slice(&b.to_be_bytes());
    v.extend_from_slice(&l.to_be_bytes());

    v
}

fn fourth_slice(buf: &[u8], idx: usize) -> u32 {
    u32::from_be_bytes(buf[idx..idx + 4].try_into().expect("4 bytes"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitfield {
    bits: Vec<u8>,
    piece_count: usize,
}

impl Bitfield {
    pub fn empty(piece_count: usize) -> Self {
        Self {
            bits: vec![0; piece_count.div_ceil(8)],
            piece_count,
        }
    }

    pub fn from_bytes(bits: Vec<u8>, piece_count: usize) -> Result<Self> {
        let data = piece_count.div_ceil(8);
        if bits.len() != data {
            bail!(
                "bitfield is {} bytes, expected {data} for {piece_count} pieces",
                bits.len()
            );
        }

        let spare = data * 8 - piece_count;
        if spare > 0 && bits[data - 1] & ((1_u8 << spare) - 1) != 0 {
            bail!("bitfield has spare trailing bit set");
        }
        Ok(Self { bits, piece_count })
    }

    pub fn has_data(&self, index: usize) -> bool {
        index < self.piece_count
            && self.bits[index / 8] & // bits[1]
            (0x80 >> (index % 8))
                != 0
    }

    pub fn set_data(&mut self, index: usize) {
        if index < self.piece_count {
            self.bits[index / 8] // stays at bits[1]
            |= 0x80 >> (index % 8);
        }
    }
}
