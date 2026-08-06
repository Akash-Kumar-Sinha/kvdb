use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};

use codec::{BincodeCodec, Codec};
use serde::{Serialize, de::DeserializeOwned};

use crate::btree::Node;
use crate::error::DbError;
use crate::page;

pub type PageId = u64;

const PAGE_SIZE: usize = 4096;

const LEN_PREFIX: usize = 4;

const CAPACITY: usize = PAGE_SIZE - LEN_PREFIX;

#[derive(Debug)]
pub struct Pager {
    file: File,
    next_page_id: PageId,
    codec: Box<dyn Codec>,
}

impl Pager {
    pub fn open(path: &str) -> Result<Self, DbError> {
        Pager::open_with(path, Box::new(BincodeCodec))
    }

    pub fn open_with(path: &str, codec: Box<dyn Codec>) -> Result<Self, DbError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let len = file.metadata()?.len();
        let next_page_id = len / PAGE_SIZE as u64;
        Ok(Pager {
            file,
            next_page_id,
            codec,
        })
    }

    pub fn codec(&self) -> &dyn Codec {
        self.codec.as_ref()
    }

    pub fn allocate_page(&mut self) -> PageId {
        let id = self.next_page_id;
        self.next_page_id += 1;
        id
    }

    pub fn read_page<S>(&mut self, id: PageId) -> Result<Node<S>, DbError>
    where
        S: DeserializeOwned,
    {
        let mut buf = vec![0u8; PAGE_SIZE];
        self.file.seek(SeekFrom::Start(id * PAGE_SIZE as u64))?;
        self.file.read_exact(&mut buf)?;
        let len = u32::from_le_bytes(
            buf[0..LEN_PREFIX]
                .try_into()
                .expect("buffer always has at least 4 bytes after read"),
        ) as usize;
        if len > CAPACITY {
            return Err(DbError::CorruptPage {
                page: id,
                len,
                capacity: CAPACITY,
            });
        }
        let data = &buf[LEN_PREFIX..LEN_PREFIX + len];

        let value = self.codec.decode(data)?;
        Ok(page::from_value(value, self.codec.name())?)
    }

    pub fn write_page<S>(&mut self, id: PageId, node: &Node<S>) -> Result<(), DbError>
    where
        S: Serialize,
    {
        let data = self.codec.encode(&page::to_value(node)?);
        if data.len() > CAPACITY {
            return Err(DbError::PageOverflow {
                len: data.len(),
                codec: self.codec.name(),
                capacity: CAPACITY,
            });
        }
        let mut buf = vec![0u8; PAGE_SIZE];
        buf[0..LEN_PREFIX].copy_from_slice(&(data.len() as u32).to_le_bytes());
        buf[LEN_PREFIX..LEN_PREFIX + data.len()].copy_from_slice(&data);
        self.file.seek(SeekFrom::Start(id * PAGE_SIZE as u64))?;
        self.file.write_all(&buf)?;
        self.file.flush()?;
        Ok(())
    }
}
