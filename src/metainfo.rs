use std::{
    path::{Component, PathBuf},
    usize,
};

use anyhow::{Context, Result, anyhow, bail};

use sha1::{Digest, Sha1};

use crate::bencode::{self, Value};

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub length: u64,
    pub offset: u64,
}

#[derive(Debug, Clone)]
pub struct MetaInfo {
    pub announce: Option<String>,
    pub announce_list: Vec<Vec<String>>,
    pub info_hash: [u8; 20],
    pub name: String,
    pub piece_length: u64,
    pub piece_hashes: Vec<[u8; 20]>,
    pub files: Vec<FileEntry>,
    pub total_length: u64,
    pub private: bool,
}

impl MetaInfo {
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        let (root, spans) =
            bencode::decode_dict(buf).context("file is not a bencoded dictionary")?;

        let info_span = spans
            .get(&b"info"[..])
            .ok_or_else(|| anyhow!("metainfo has no 'info' dictionary"))?;
        let info_hash = sha1(&buf[info_span.clone()]);

        let info = root
            .get(&b"info"[..])
            .and_then(Value::as_dict)
            .ok_or_else(|| anyhow!("'info' is not a dictionary"))?;

        let name = info
            .get(&b"name"[..])
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("'info.name' missing"))?
            .to_string();

        let piece_length = info
            .get(&b"piece length"[..])
            .and_then(Value::as_int)
            .filter(|n| *n > 0)
            .ok_or_else(|| anyhow!("'info.piece length' missing or not positive"))?
            as u64;

        let pieces = info
            .get(&b"pieces"[..])
            .and_then(Value::as_bytes)
            .ok_or_else(|| anyhow!("'info.pieces' missing"))?;

        if pieces.len() % 20 != 0 {
            bail!(
                "'info.pieces' is {} bytes, not a multiple of 20",
                pieces.len()
            );
        }

        let piece_hashes: Vec<[u8; 20]> = pieces
            .chunks_exact(20)
            .map(|c| c.try_into().expect("chunk is exactly 20 bytes"))
            .collect();

        let files = match (info.get(&b"length"[..]), info.get(&b"files"[..])) {
            (Some(len), None) => {
                let length = len
                    .as_int()
                    .filter(|n| *n >= 0)
                    .ok_or_else(|| anyhow!("'info.length' is not a valid size"))?
                    as u64;

                vec![FileEntry {
                    path: PathBuf::from(&name),
                    length,
                    offset: 0,
                }]
            }
            (None, Some(files)) => parse_file_list(files)?,
            (Some(_), Some(_)) => bail!("'info' has both 'length' and 'files'"),
            (None, None) => bail!("'info' has neither 'length' nor 'files'"),
        };

        let total_length: u64 = files.iter().map(|f| f.length).sum();

        let expected_pieces = total_length.div_ceil(piece_length) as usize;

        if piece_hashes.len() != expected_pieces {
            bail!(
                "torrent needs {} pieces but {} bytes at {} bytes/piece need {}",
                piece_hashes.len(),
                total_length,
                piece_length,
                expected_pieces
            );
        }

        let announce = root
            .get(&b"announce"[..])
            .and_then(Value::as_str)
            .map(str::to_string);

        let announce_list = root
            .get(&b"announce-list"[..])
            .and_then(Value::as_list)
            .map(|v| {
                v.iter()
                    .filter_map(Value::as_list)
                    .map(|d| {
                        d.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .collect()
            })
            .unwrap_or_default();

        let private = info
            .get(&b"private"[..])
            .and_then(Value::as_int)
            .is_some_and(|v| v == 1);

        Ok(MetaInfo {
            announce,
            announce_list,
            info_hash,
            name,
            piece_length,
            piece_hashes,
            files,
            total_length,
            private,
        })
    }

    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = path.as_ref();
        let buf = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;

        Self::from_bytes(&buf).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn piece_size(&self, index: usize) -> u64 {
        let start = index as u64 * self.piece_length;

        self.piece_length.min(self.total_length - start)
    }

    pub fn is_single_file(&self) -> bool {
        self.files.len() == 1 && self.files[0].path.as_os_str() == self.name.as_str()
    }

    pub fn trackers(&self) -> Vec<&str> {
        let mut data = Vec::new();

        for url in self
            .announce_list
            .iter()
            .flatten()
            .map(String::as_str)
            .chain(self.announce.as_deref())
        {
            if !data.contains(&url) {
                data.push(url);
            }
        }

        data
    }

    pub fn info_hash_hex(&self) -> String {
        crate::hex(&self.info_hash)
    }

    pub fn magnet(&self) -> String {
        let mut s = format!(
            "magnet:?xt=urn:btih:{}&dn={}",
            self.info_hash_hex(),
            crate::urlencode(self.name.as_bytes())
        );

        for tracker in self.trackers() {
            s.push_str("&tr=");
            s.push_str(&crate::urlencode(tracker.as_bytes()));
        }

        s
    }
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let digest = Sha1::digest(data);
    let mut out = [0_u8; 20];

    out.copy_from_slice(&digest);
    out
}

fn parse_file_list(files: &Value) -> Result<Vec<FileEntry>> {
    let list = files
        .as_list()
        .ok_or_else(|| anyhow!("'info.files' is not a list"))?;

    if list.is_empty() {
        bail!("'info.files' is empty");
    }

    let mut out = Vec::with_capacity(list.len());
    let mut offset = 0_u64;

    for (i, entry) in list.iter().enumerate() {
        let length = entry
            .get("length")
            .and_then(Value::as_int)
            .filter(|n| *n >= 0)
            .ok_or_else(|| anyhow!("file {i} has no valid length"))? as u64;

        let components = entry
            .get("path")
            .and_then(Value::as_list)
            .ok_or_else(|| anyhow!("file {i} has no path list"))?;

        let mut path = PathBuf::new();
        for component in components {
            let part = component
                .as_str()
                .ok_or_else(|| anyhow!("file {i} has an invalid path component"))?;

            let mut components = std::path::Path::new(part).components();

            match (components.next(), components.next()) {
                (Some(Component::Normal(name)), None) => path.push(name),
                _ => bail!("file {i} has an unsafe path component: {part:?}"),
            }
        }

        if path.as_os_str().is_empty() {
            bail!("file {i} has an empty path");
        }

        out.push(FileEntry {
            path,
            offset,
            length,
        });

        offset += length;
    }

    Ok(out)
}
