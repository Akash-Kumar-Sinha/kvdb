//! The B+tree storage engine: the typestate-checked [`BTree`], the page-based
//! [`Pager`], and [`DbError`], the storage-level error the rest of the
//! workspace propagates.
//!
//! This is an internal crate rather than KvDB's public entry point — `kvdb`
//! re-exports the handful of items an ordinary caller needs. See the
//! workspace README's "Why a B+tree, not a B-tree" section for why the type
//! here is still named `BTree` despite no longer being a classic B-tree.

mod btree;
mod error;
mod page;
mod pager;

pub use btree::{BTree, Initialized, Locked, Node, PagerState, Uninitialized, Unlocked};
pub use error::DbError;
pub use pager::{PageId, Pager};
pub use value::{Value, ValueError};

#[cfg(test)]
mod invariants {
    use super::*;

    type Tree = BTree<i64, Initialized, Unlocked>;

    fn check_node(
        pager: &mut Pager,
        id: PageId,
        lower: Option<&i64>,
        upper: Option<&i64>,
        depth: usize,
        leaf_depth: &mut Option<usize>,
        leaves: &mut Vec<PageId>,
    ) -> Result<(), DbError> {
        let node: Node<i64> = pager.read_page(id)?;

        assert!(
            node.keys.windows(2).all(|pair| pair[0] <= pair[1]),
            "page {id} holds unsorted keys"
        );
        for key in &node.keys {
            if let Some(low) = lower {
                assert!(
                    key >= low,
                    "page {id}: key {key} escapes its lower bound {low}"
                );
            }
            if let Some(high) = upper {
                assert!(
                    key < high,
                    "page {id}: key {key} escapes its upper bound {high}"
                );
            }
        }

        if node.is_leaf {
            assert!(node.children.is_empty(), "leaf {id} has children");
            assert_eq!(
                node.values.len(),
                node.keys.len(),
                "leaf {id} has a key without a value"
            );
            match leaf_depth {
                Some(expected) => assert_eq!(*expected, depth, "leaf {id} is at the wrong depth"),
                None => *leaf_depth = Some(depth),
            }
            leaves.push(id);
            return Ok(());
        }

        assert!(
            node.values.is_empty(),
            "internal page {id} stores values — the whole point of a B+tree is that it does not"
        );
        assert_eq!(node.next, None, "internal page {id} is on the leaf chain");
        assert_eq!(
            node.children.len(),
            node.keys.len() + 1,
            "internal page {id} has the wrong number of children"
        );

        for (i, &child) in node.children.iter().enumerate() {
            let low = if i == 0 {
                lower
            } else {
                Some(&node.keys[i - 1])
            };
            let high = if i == node.keys.len() {
                upper
            } else {
                Some(&node.keys[i])
            };
            check_node(pager, child, low, high, depth + 1, leaf_depth, leaves)?;
        }
        Ok(())
    }

    fn leaf_chain(pager: &mut Pager, root_id: PageId) -> Result<Vec<PageId>, DbError> {
        let mut id = root_id;
        loop {
            let node: Node<i64> = pager.read_page(id)?;
            if node.is_leaf {
                break;
            }
            id = node.children[0];
        }

        let mut chain = Vec::new();
        let mut next = Some(id);
        while let Some(page) = next {
            chain.push(page);
            next = pager.read_page::<i64>(page)?.next;
        }
        Ok(chain)
    }

    fn assert_bplus_tree(tree: &Tree) -> Result<usize, DbError> {
        let mut guard = tree.pager_state().acquire();
        let root_id = guard.root_id;

        let mut leaf_depth = None;
        let mut leaves = Vec::new();
        check_node(
            &mut guard.pager,
            root_id,
            None,
            None,
            0,
            &mut leaf_depth,
            &mut leaves,
        )?;

        let chain = leaf_chain(&mut guard.pager, root_id)?;
        assert_eq!(
            chain, leaves,
            "the sibling chain must visit leaves in left-to-right order"
        );
        Ok(leaf_depth.expect("a tree always has at least one leaf"))
    }

    #[test]
    fn values_live_only_in_leaves_and_the_chain_links_them() -> Result<(), DbError> {
        let path = "/tmp/test_bplus_invariants.db";
        std::fs::remove_file(path).ok();

        let mut tree = BTree::<i64, Uninitialized, Locked>::new(path)?.unlock();
        for i in 0..500 {
            tree.put(i, i * 10)?;
        }

        let depth = assert_bplus_tree(&tree)?;
        assert!(
            depth >= 2,
            "500 keys should build at least 3 levels, got {depth}"
        );

        let keys: Vec<i64> = tree.range()?.into_iter().map(|(key, _)| key).collect();
        assert_eq!(
            keys,
            (0..500).collect::<Vec<_>>(),
            "the leaf walk lost order"
        );

        std::fs::remove_file(path).ok();
        Ok(())
    }

    #[test]
    fn the_chain_survives_the_merge_and_borrow_paths() -> Result<(), DbError> {
        let path = "/tmp/test_bplus_delete_invariants.db";
        std::fs::remove_file(path).ok();

        let mut tree = BTree::<i64, Uninitialized, Locked>::new(path)?.unlock();
        for i in 0..400 {
            tree.put(i, i * 10)?;
        }

        for i in (0..400).step_by(3) {
            assert!(tree.delete(i)?.0, "delete({i}) reported not found");
            assert_bplus_tree(&tree)?;
        }

        let survivors: Vec<i64> = (0..400).filter(|i| i % 3 != 0).collect();
        let keys: Vec<i64> = tree.range()?.into_iter().map(|(key, _)| key).collect();
        assert_eq!(keys, survivors, "deletes broke the leaf chain");
        assert_eq!(tree.len()?, survivors.len());

        for key in &survivors {
            assert_eq!(
                tree.get::<i64>(key)?,
                key * 10,
                "survivor {key} was corrupted"
            );
        }

        std::fs::remove_file(path).ok();
        Ok(())
    }

    #[test]
    fn keys_arriving_out_of_order_still_land_in_a_sorted_tree() -> Result<(), DbError> {
        let path = "/tmp/test_bplus_shuffled.db";
        std::fs::remove_file(path).ok();

        let mut tree = BTree::<i64, Uninitialized, Locked>::new(path)?.unlock();
        for i in 0..300i64 {
            let key = (i * 7) % 300;
            tree.put(key, key * 10)?;
        }

        assert_bplus_tree(&tree)?;
        let keys: Vec<i64> = tree.range()?.into_iter().map(|(key, _)| key).collect();
        assert_eq!(
            keys,
            (0..300).collect::<Vec<_>>(),
            "a shuffled insert order left the tree unsorted"
        );

        std::fs::remove_file(path).ok();
        Ok(())
    }
}

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
            next: None,
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
