mod btree;
mod error;
mod page;
mod pager;

pub use btree::{BTree, Initialized, Locked, Node, PagerState, Uninitialized, Unlocked};
pub use error::DbError;
pub use pager::{PageId, Pager};
pub use value::{Value, ValueError};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insertion_structure() -> Result<(), DbError> {
        let path = "/tmp/test_btree.db";
        let tree = BTree::<i64, Uninitialized, Locked>::new(path)?;
        let mut tree = tree.unlock();

        for i in 1..=12 {
            tree.put(i, i * 10)?;
        }

        for i in 1..=12 {
            let value: i64 = tree.get(&i)?;
            assert_eq!(value, i * 10, "get({i}) mismatch");
        }

        assert!(
            tree.get::<i64>(&999).is_err_and(|err| err.is_not_found()),
            "missing key should return NotFound"
        );

        let range_i64: Vec<(i64, i64)> = tree
            .range()?
            .into_iter()
            .map(|(k, v)| (k, i64::try_from(v).expect("expected int")))
            .collect();
        let expected: Vec<(i64, i64)> = (1..=12).map(|i| (i, i * 10)).collect();
        assert_eq!(
            range_i64, expected,
            "range() must return sorted (key, value) pairs"
        );

        assert_eq!(tree.len()?, 12);

        let (found, value) = tree.delete(12)?;
        assert!(found, "delete(12) should report found = true");
        let value = i64::try_from(value.expect("value missing")).expect("expected int");
        assert_eq!(value, 120);

        assert!(
            tree.get::<i64>(&12).is_err_and(|err| err.is_not_found()),
            "12 should be gone after delete"
        );
        assert_eq!(tree.len()?, 11);

        let (found, value) = tree.delete(999)?;
        assert!(!found, "deleting a missing key should report found = false");
        assert!(value.is_none());
        assert_eq!(
            tree.len()?,
            11,
            "len should be unchanged after a failed delete"
        );

        let (found, value) = tree.delete(5)?;
        assert!(found);
        let value: i64 = value
            .expect("value missing")
            .try_into()
            .expect("expected int");
        assert_eq!(value, 50);

        assert!(tree.get::<i64>(&5).is_err_and(|err| err.is_not_found()));
        assert_eq!(tree.len()?, 10);

        let mut tree = tree.lock();
        let value: i64 = tree.get(&1)?;
        assert_eq!(value, 10);

        std::fs::remove_file(path).ok();
        Ok(())
    }

    #[test]
    fn test_storage_failures_are_errors() -> Result<(), DbError> {
        let path = "/tmp/test_btree_failures.db";
        std::fs::remove_file(path).ok();

        let mut tree = BTree::<i64, Uninitialized, Locked>::new(path)?.unlock();
        let oversized = Value::Bytes(vec![0u8; 8192]);
        assert!(
            matches!(
                tree.put(1, oversized),
                Err(DbError::PageOverflow {
                    codec: "bincode",
                    ..
                })
            ),
            "a node larger than a page must report PageOverflow"
        );

        let mut pager = Pager::open(path)?;
        assert!(
            matches!(pager.read_page::<i64>(9999), Err(DbError::Io(_))),
            "reading past the end of the file must report Io"
        );

        std::fs::remove_file(path).ok();
        Ok(())
    }

    #[test]
    fn test_reading_with_the_wrong_codec_is_an_error() -> Result<(), DbError> {
        let path = "/tmp/test_btree_wrong_codec.db";
        std::fs::remove_file(path).ok();

        let node: Node<i64> = Node {
            keys: vec![1],
            values: vec![Value::I64(10)],
            children: Vec::new(),
            is_leaf: true,
        };
        let mut written = Pager::open(path)?;
        written.write_page(0, &node)?;
        drop(written);

        let mut misread = Pager::open_with(path, Box::new(codec::JsonCodec))?;
        let err = misread
            .read_page::<i64>(0)
            .expect_err("bincode pages are not json");
        assert!(
            matches!(
                err,
                DbError::Value(ValueError::Decode { codec: "json", .. })
            ),
            "expected a json decode error, got {err:?}"
        );

        std::fs::remove_file(path).ok();
        Ok(())
    }

    #[test]
    fn test_corrupt_length_prefix_is_an_error() -> Result<(), DbError> {
        use std::io::Write;

        let path = "/tmp/test_btree_corrupt_prefix.db";
        std::fs::remove_file(path).ok();

        let mut page = vec![0u8; 4096];
        page[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::File::create(path)?.write_all(&page)?;

        let mut pager = Pager::open(path)?;
        assert!(
            matches!(
                pager.read_page::<i64>(0),
                Err(DbError::CorruptPage { page: 0, .. })
            ),
            "an impossible length prefix must report CorruptPage"
        );

        std::fs::remove_file(path).ok();
        Ok(())
    }
}
