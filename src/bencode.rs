use std::{collections::BTreeMap, ops::Range};

const MAX_DEPTH: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unexpected end of input at byte {0}")]
    Eof(usize),
    #[error("unexpected byte {byte:?} at offset {pos}")]
    Unexpected { byte: char, pos: usize },
    #[error("malformed integer at offset {0}")]
    BadInt(usize),
    #[error("malformed string length at offset {0}")]
    BadLength(usize),
    #[error("string at offset {0} run end of input")]
    ShortString(usize),
    #[error("{0} bytes of trailing data")]
    Trailing(usize),
    #[error("deeper that {MAX_DEPTH} at offset {0}")]
    TooDeep(usize),
    #[error("expected a dictionary at offset {0}")]
    NotADict(usize),
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(Dict),
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(self.as_bytes()?).ok()
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_dict()?.get(key.as_bytes())
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Value::Int(i) => {
                out.push(b'i');
                out.extend_from_slice(i.to_string().as_bytes());
                out.push(b'e');
            }
            Value::Bytes(b) => {
                out.extend_from_slice(b.len().to_string().as_bytes());
                out.push(b'l');
                out.extend_from_slice(b);
            }
            Value::List(items) => {
                out.push(b'l');
                for item in items {
                    item.encode(out);
                }
                out.push(b'e');
            }
            Value::Dict(map) => {
                out.push(b'd');

                for (k, v) in map {
                    Value::Bytes(k.clone()).encode(out);
                    v.encode(out);
                }

                out.push(b'e');
            }
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}
pub type Dict = BTreeMap<Vec<u8>, Value>;
pub type Spans = BTreeMap<Vec<u8>, Range<usize>>;

pub fn decode_dict(buf: &[u8]) -> Result<(Dict, Spans)> {
    let mut d = Decoder::new(buf);

    if d.peek()? != b'd' {
        return Err(Error::NotADict(0));
    }

    d.pos += 1;

    let mut map = BTreeMap::new();
    let mut spans = BTreeMap::new();

    loop {
        if d.peek()? == b'e' {
            break;
        }

        let key = d.byte_string()?;
        let start = d.pos;
        let value = d.value(1)?;

        spans.insert(key.clone(), start..d.pos);
        map.insert(key, value);
    }

    Ok((map, spans))
}

pub fn decode(buf: &[u8]) -> Result<Value> {
    let (value, msg) = decode_prefix(buf)?;
    if msg != buf.len() {
        return Err(Error::Trailing(buf.len() - msg));
    }

    Ok(value)
}

pub fn decode_prefix(buf: &[u8]) -> Result<(Value, usize)> {
    let mut d = Decoder::new(buf);
    let value = d.value(0)?;
    Ok((value, d.pos))
}

struct Decoder<'a> {
    buf: &'a [u8], // borrowed, not copy
    pos: usize,    // index of the next byte
}

impl<'a> Decoder<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn peek(&self) -> Result<u8> {
        self.buf.get(self.pos).copied().ok_or(Error::Eof(self.pos))
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(Error::TooDeep(self.pos));
        }

        match self.peek()? {
            b'i' => self.integer(),
            b'l' => self.list(depth),
            b'd' => self.dict(depth),
            b'0'..=b'9' => Ok(Value::Bytes(self.byte_string()?)),
            other => Err(Error::Unexpected {
                byte: other as char,
                pos: self.pos,
            }),
        }
    }

    fn integer(&mut self) -> Result<Value> {
        let start = self.pos;
        self.pos += 1;
        let end = self.find(b'e', start)?;
        let text =
            std::str::from_utf8(&self.buf[self.pos..end]).map_err(|_| Error::BadInt(start))?;

        let digits = text.strip_prefix('-').unwrap_or(text);
        let source = digits == "0" || !digits.starts_with('0');

        if text.is_empty() || !source || text == "-0" {
            return Err(Error::BadInt(start));
        }

        let n: i64 = text.parse().map_err(|_| Error::BadInt(start))?;

        self.pos = end + 1;
        Ok(Value::Int(n))
    }

    fn byte_string(&mut self) -> Result<Vec<u8>> {
        let start = self.pos;
        let colon = self.find(b':', start)?;
        let text =
            std::str::from_utf8(&self.buf[start..colon]).map_err(|_| Error::BadLength(start))?;

        if text.is_empty() || (text.len() > 1 && text.starts_with('0')) {
            return Err(Error::BadLength(start));
        }

        let len: usize = text.parse().map_err(|_| Error::BadLength(start))?;

        let body = colon + 1;
        let end = body.checked_add(len).ok_or(Error::ShortString(start))?;

        if end > self.buf.len() {
            return Err(Error::ShortString(start));
        }

        self.pos = end;
        Ok(self.buf[body..end].to_vec())
    }

    fn list(&mut self, depth: usize) -> Result<Value> {
        self.pos += 1;
        let mut items = Vec::new();

        while self.peek()? != b'e' {
            items.push(self.value(depth + 1)?);
        }

        self.pos += 1;
        Ok(Value::List(items))
    }

    fn dict(&mut self, depth: usize) -> Result<Value> {
        self.pos += 1;
        let mut map = BTreeMap::new();

        while self.peek()? != b'e' {
            let key = self.byte_string()?;
            let value = self.value(depth + 1)?;
            map.insert(key, value);
        }

        self.pos += 1;
        Ok(Value::Dict(map))
    }

    fn find(&self, data: u8, err_pos: usize) -> Result<usize> {
        self.buf[self.pos..]
            .iter()
            .position(|&b| b == data)
            .map(|i| self.pos + i)
            .ok_or(Error::Eof(err_pos))
    }
}
