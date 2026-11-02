use std::{
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::metainfo::{MetaInfo, sha1};
use crate::peer::Bitfield;

struct OpenFile {
    file: File,
    path: PathBuf,
    offset: u64,
    length: u64,
}

pub struct Storage {
    files: Vec<OpenFile>,
    piece_length: u64,
    total_length: u64,
    piece_count: usize,
}

impl Storage {
    pub fn create(root: impl AsRef<Path>, meta: &MetaInfo) -> Result<Self> {
        let root = root.as_ref();
        let base = if meta.is_single_file() {
            root.to_path_buf()
        } else {
            root.join(&meta.name)
        };

        let mut files = Vec::with_capacity(meta.files.len());

        for entry in &meta.files {
            let path = base.join(&entry.path);

            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }

            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .with_context(|| format!("opening {}", path.display()))?;

            file.set_len(entry.length)
                .with_context(|| format!("sizing {} to {} bytes", path.display(), entry.length))?;

            files.push(OpenFile {
                file,
                path,
                offset: entry.offset,
                length: entry.length,
            });
        }

        Ok(Self {
            files,
            piece_count: meta.piece_count(),
            piece_length: meta.piece_length,
            total_length: meta.total_length,
        })
    }

    pub fn piece_size(&self, index: usize) -> u64 {
        let start = index as u64 * self.piece_length;

        self.piece_length.min(self.total_length - start)
    }

    fn map_buffer_chunks(
        &self,
        start: u64,
        len: u64,
    ) -> impl Iterator<Item = (&OpenFile, u64, std::ops::Range<usize>)> {
        let end = start + len;

        self.files.iter().filter_map(move |f| {
            let file_end = f.offset + f.length;

            let from = start.max(f.offset);
            let to = end.min(file_end);

            if from >= to {
                return None;
            }

            let buf_range = (from - start) as usize..(to - start) as usize;

            Some((f, from - f.offset, buf_range))
        })
    }

    pub fn read_piece(&self, index: usize, out: &mut Vec<u8>) -> Result<()> {
        let size = self.piece_size(index) as usize;

        out.clear();
        out.resize(size, 0);

        let start = index as u64 * self.piece_length;

        for (file, file_offset, range) in self.map_buffer_chunks(start, size as u64) {
            read_at(&file.file, &mut out[range], file_offset)
                .with_context(|| format!("reading piece {index} from {}", file.path.display()))?;
        }

        Ok(())
    }

    pub fn write_piece(&self, index: usize, data: &[u8]) -> Result<()> {
        self.check_piece(index, data.len())?;

        let start = index as u64 * self.piece_length;

        for (file, file_offset, range) in self.map_buffer_chunks(start, data.len() as u64) {
            write_at(&file.file, &data[range], file_offset)
                .with_context(|| format!("writing piece {index} into {}", file.path.display()))?;
        }

        Ok(())
    }

    fn check_piece(&self, index: usize, len: usize) -> Result<()> {
        if index >= self.piece_count {
            bail!(
                "
            piece {index} is out of range (have {} pieces)
                ",
                self.piece_count
            );
        }

        let expected = self.piece_size(index) as usize;
        if len != expected {
            bail!(
                "
            piece {index} is {len} bytes, expected {expected}
                "
            );
        }
        Ok(())
    }

    pub fn resume_download(
        &self,
        meta: &MetaInfo,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<Bitfield> {
        let mut have = Bitfield::empty(self.piece_count);
        let mut buf = Vec::with_capacity(self.piece_length as usize);

        for index in 0..self.piece_count {
            self.read_piece(index, &mut buf)?;

            if sha1(&buf) == meta.piece_hashes[index] {
                have.set_data(index);
            }

            progress(index + 1, self.piece_count);
        }

        Ok(have)
    }

    pub fn flush_file(&self) -> Result<()> {
        for f in &self.files {
            f.file
                .sync_all()
                .with_context(|| format!("syncing {}", f.path.display()))?;
        }

        Ok(())
    }
}

#[cfg(unix)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}
#[cfg(unix)]
fn write_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}
