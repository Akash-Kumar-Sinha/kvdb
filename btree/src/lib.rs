mod btree;
mod error;
mod pager;
mod value;

pub use btree::{BTree, Initialized, Locked, Node, PagerState, Uninitialized, Unlocked};
pub use error::ValueError;
pub use pager::{PageId, Pager};
pub use value::Value;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pager::Pager;

    #[cfg(test)]
    fn fresh_pager(path: &str) -> Pager {
        std::fs::remove_file(path).ok();
        Pager::open(path).expect("open failed")
    }

    #[test]
    fn test_insertion_structure() {
        let path = "/tmp/test_btree.db";
        let tree = BTree::<i64, Uninitialized, Locked>::new(path);
        let mut tree = tree.unlock();

        for i in 1..=12 {
            tree.put(i, i * 10);
        }

        for i in 1..=12 {
            let value: i64 = tree.get(&i).expect("get failed");
            assert_eq!(value, i * 10, "get({i}) mismatch");
        }

        assert!(
            matches!(tree.get::<i64>(&999), Err(ValueError::NotFound)),
            "missing key should return NotFound"
        );

        let range = tree.range();
        let range_i64: Vec<(i64, i64)> = range
            .into_iter()
            .map(|(k, v)| (k, i64::try_from(v).expect("expected int")))
            .collect();
        let expected: Vec<(i64, i64)> = (1..=12).map(|i| (i, i * 10)).collect();
        assert_eq!(
            range_i64, expected,
            "range() must return sorted (key, value) pairs"
        );

        assert_eq!(tree.len(), 12);

        let (found, value) = tree.delete(12);
        assert!(found, "delete(12) should report found = true");
        let value = i64::try_from(value.expect("value missing")).expect("expected int");
        assert_eq!(value, 120);

        assert!(
            matches!(tree.get::<i64>(&12), Err(ValueError::NotFound)),
            "12 should be gone after delete"
        );
        assert_eq!(tree.len(), 11);

        let (found, value) = tree.delete(999);
        assert!(!found, "deleting a missing key should report found = false");
        assert!(value.is_none());
        assert_eq!(
            tree.len(),
            11,
            "len should be unchanged after a failed delete"
        );

        let (found, value) = tree.delete(5);
        assert!(found);
        let value: i64 = value
            .expect("value missing")
            .try_into()
            .expect("expected int");
        assert_eq!(value, 50);

        assert!(matches!(tree.get::<i64>(&5), Err(ValueError::NotFound)));
        assert_eq!(tree.len(), 10);

        let mut tree = tree.lock();
        let value: i64 = tree.get(&1).expect("get failed");
        assert_eq!(value, 10);

        std::fs::remove_file(path).ok();
    }
}
