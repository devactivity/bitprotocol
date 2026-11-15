use std::collections::HashMap;

use crate::{
    metainfo::MetaInfo,
    peer::{BLOCK_SIZE, Bitfield},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeerKey(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRequest {
    pub index: u32,
    pub begin: u32,
    pub length: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockState {
    Pending,
    InFlight(PeerKey),
    Received,
}

struct Partial {
    buf: Vec<u8>,
    blocks: Vec<BlockState>,
    received: usize,
}

impl Partial {
    fn new(size: u64) -> Self {
        let block_count = size.div_ceil(BLOCK_SIZE as u64) as usize;

        Self {
            buf: vec![0; size as usize],
            blocks: vec![BlockState::Pending; block_count],
            received: 0,
        }
    }

    fn block_size(&self, block: usize) -> u32 {
        let start = block as u64 * BLOCK_SIZE as u64;

        (self.buf.len() as u64 - start).min(BLOCK_SIZE as u64) as u32
    }

    fn is_complete(&self) -> bool {
        self.received == self.blocks.len()
    }
}

#[derive(Debug)]
pub enum BlockOutcome {
    Ignored,
    Accepted,
    PieceComplete { index: usize, data: Vec<u8> },
}

pub struct Picker {
    piece_count: usize,
    piece_length: u64,
    total_length: u64,
    data: Bitfield,
    availability: Vec<u32>,
    partials: HashMap<usize, Partial>,
    end_data: bool,
    pub corrupt_pieces: u64,
}

impl Picker {
    pub fn new(meta: &MetaInfo, data: Bitfield) -> Self {
        Self {
            piece_count: meta.piece_count(),
            piece_length: meta.piece_length,
            total_length: meta.total_length,
            data,
            availability: vec![0; meta.piece_count()],
            partials: HashMap::new(),
            end_data: false,
            corrupt_pieces: 0,
        }
    }

    pub fn piece_size(&self, index: usize) -> u64 {
        let start = index as u64 * self.piece_length;

        self.piece_length.min(self.total_length - start)
    }

    pub fn data(&self) -> &Bitfield {
        &self.data
    }

    pub fn is_complete(&self) -> bool {
        self.data.is_complete()
    }

    pub fn is_end_data(&self) -> bool {
        self.end_data
    }

    pub fn completed_bytes(&self) -> u64 {
        (0..self.piece_count)
            .filter(|i| self.data.has_data(*i))
            .map(|i| self.piece_size(i))
            .sum()
    }

    pub fn peer_joined(&mut self, bits: &Bitfield) {
        for i in 0..self.piece_count.min(bits.piece_count()) {
            if bits.has_data(i) {
                self.availability[i] += 1;
            }
        }
    }

    pub fn peer_left(&mut self, bits: &Bitfield) {
        for i in 0..self.piece_count.min(bits.piece_count()) {
            if bits.has_data(i) {
                self.availability[i] = self.availability[i].saturating_sub(1);
            }
        }
    }

    pub fn peer_has_data(&mut self, index: usize) {
        if index < self.piece_count {
            self.availability[index] += 1;
        }
    }

    pub fn pick(
        &mut self,
        peer: PeerKey,
        peer_data: &Bitfield,
        picked: usize,
    ) -> Vec<BlockRequest> {
        let mut out = Vec::new();
        if picked == 0 || self.is_complete() {
            return out;
        }

        let mut started: Vec<usize> = self
            .partials
            .iter()
            .filter(|(index, p)| peer_data.has_data(**index) && !p.is_complete())
            .map(|(index, _)| *index)
            .collect();

        started.sort_unstable_by_key(|i| std::cmp::Reverse(self.partials[i].received));

        for index in started {
            self.pending_data(index, peer, picked, &mut out);

            if out.len() >= picked {
                return out;
            }
        }

        while let Some(index) = self.unstarted(peer_data) {
            self.partials
                .insert(index, Partial::new(self.piece_size(index)));

            self.pending_data(index, peer, picked, &mut out);

            if out.len() >= picked {
                return out;
            }
        }

        if out.is_empty() {
            self.end_data = true;
            self.take_end_data(peer, peer_data, picked, &mut out);
        }

        out
    }

    fn take_end_data(
        &mut self,
        peer: PeerKey,
        peer_data: &Bitfield,
        picked: usize,
        out: &mut Vec<BlockRequest>,
    ) {
        let mut indices: Vec<usize> = self
            .partials
            .keys()
            .copied()
            .filter(|i| peer_data.has_data(*i))
            .collect();

        indices.sort_unstable();

        for index in indices {
            let partial = &self.partials[&index];

            for block in 0..partial.blocks.len() {
                if out.len() >= picked {
                    return;
                }

                if matches!(
                    partial.blocks[block],
                    BlockState::InFlight(owner) if owner != peer
                ) {
                    out.push(BlockRequest {
                        index: index as u32,
                        begin: block as u32 * BLOCK_SIZE,
                        length: partial.block_size(block),
                    });
                }
            }
        }
    }

    fn unstarted(&self, peer_data: &Bitfield) -> Option<usize> {
        (0..self.piece_count)
            .filter(|i| {
                !self.data.has_data(*i) && peer_data.has_data(*i) && !self.partials.contains_key(i)
            })
            .min_by_key(|i| self.availability[*i])
    }

    fn pending_data(
        &mut self,
        index: usize,
        peer: PeerKey,
        picked: usize,
        out: &mut Vec<BlockRequest>,
    ) {
        let Some(partial) = self.partials.get_mut(&index) else {
            return;
        };

        for block in 0..partial.blocks.len() {
            if out.len() >= picked {
                return;
            }

            if partial.blocks[block] == BlockState::Pending {
                partial.blocks[block] == BlockState::InFlight(peer);

                out.push(BlockRequest {
                    index: index as u32,
                    begin: block as u32 * BLOCK_SIZE,
                    length: partial.block_size(block),
                });
            }
        }
    }

    pub fn block_received(&mut self, index: usize, begin: u32, data: &[u8]) -> BlockOutcome {
        let Some(partial) = self.partials.get_mut(&index) else {
            return BlockOutcome::Ignored;
        };

        if !begin.is_multiple_of(BLOCK_SIZE) {
            return BlockOutcome::Ignored;
        }

        let block = (begin / BLOCK_SIZE) as usize;

        if block >= partial.blocks.len()
            || partial.blocks[block] == BlockState::Received
            || data.len() != partial.block_size(block) as usize
        {
            return BlockOutcome::Ignored;
        }

        let start = begin as usize;

        partial.buf[start..start + data.len()].copy_from_slice(data);
        partial.blocks[block] = BlockState::Received;
        partial.received += 1;

        if partial.is_complete() {
            let data = std::mem::take(&mut partial.buf);

            BlockOutcome::PieceComplete { index, data }
        } else {
            BlockOutcome::Accepted
        }
    }

    pub fn piece_verified(&mut self, index: usize) {
        self.data.set_data(index);
        self.partials.remove(&index);
    }

    pub fn piece_corrupt(&mut self, index: usize) {
        self.corrupt_pieces += 1;
        self.partials.remove(&index);
    }

    pub fn release_peer(&mut self, peer: PeerKey) {
        for partial in self.partials.values_mut() {
            for state in &mut partial.blocks {
                if *state == BlockState::InFlight(peer) {
                    *state = BlockState::Pending;
                }
            }
        }
    }

    pub fn release_stale(&mut self, peer: PeerKey) {
        self.release_peer(peer);
    }
}
