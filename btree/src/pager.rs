use serde::{Serialize, de::DeserializeOwned};
use std::fs::{File, OpenOptions};
use std::io::{Read, Result as IoResult, Seek, SeekFrom, Write};

use crate::btree::Node;

pub type PageId = u64;
const PAGE_SIZE: usize = 4096;

pub struct Pager {
    file: File,
    next_page_id: PageId,
}

impl Pager {
    pub fn open(path: &str) -> IoResult<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let len = file.metadata()?.len();
        let next_page_id = len / PAGE_SIZE as u64;
        Ok(Pager { file, next_page_id })
    }

    pub fn allocate_page(&mut self) -> PageId {
        let id = self.next_page_id;
        self.next_page_id += 1;
        id
    }

    pub fn read_page<S>(&mut self, id: PageId) -> IoResult<Node<S>>
    where
        S: DeserializeOwned,
    {
        let mut buf = vec![0u8; PAGE_SIZE];
        self.file.seek(SeekFrom::Start(id * PAGE_SIZE as u64))?;
        self.file.read_exact(&mut buf)?;
        let len = u32::from_le_bytes(
            buf[0..4]
                .try_into()
                .expect("buffer always has at least 4 bytes after read"),
        ) as usize;
        let data = &buf[4..4 + len];
        let node: Node<S> =
            bincode::deserialize(data).expect("corrupt page — see note on checksums below");
        Ok(node)
    }

    pub fn write_page<S>(&mut self, id: PageId, node: &Node<S>) -> IoResult<()>
    where
        S: Serialize,
    {
        let data = bincode::serialize(node).expect("serialize failed");
        assert!(
            data.len() + 4 <= PAGE_SIZE,
            "node too large for one page ({} bytes) — lower MIN_DEGREE \
             or this key/value type is too big to store inline",
            data.len()
        );
        let mut buf = vec![0u8; PAGE_SIZE];
        buf[0..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        buf[4..4 + data.len()].copy_from_slice(&data);
        self.file.seek(SeekFrom::Start(id * PAGE_SIZE as u64))?;
        self.file.write_all(&buf)?;
        self.file.flush()?;
        Ok(())
    }
}
