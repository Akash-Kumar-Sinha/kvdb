use crate::btree::{BTree, Initialized, Locked, Uninitialized, Unlocked};
use serde::{de::DeserializeOwned, Serialize};

pub struct KvDb<S, T>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
    T: Clone + Serialize + DeserializeOwned,
{
    inner: BTree<S, T, Initialized, Unlocked>,
}

impl<S, T> KvDb<S, T>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
    T: Clone + Serialize + DeserializeOwned,
{
    pub fn open(path: &str) -> Self {
        let tree = BTree::<S, T, Uninitialized, Locked>::new(path);
        KvDb { inner: tree.unlock() }
    }

    pub fn get(&mut self, key: &S) -> Option<T> {
        self.inner.get(key)
    }

    pub fn put(&mut self, key: S, value: T) {
        self.inner.put(key, value);
    }

    pub fn delete(&mut self, key: S) -> (bool, Option<T>) {
        self.inner.delete(key)
    }

    pub fn range(&mut self) -> Vec<(S, T)> {
        self.inner.range()
    }

    pub fn len(&mut self) -> usize {
        self.inner.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_path(path: &str) {
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_kvdb_basic_usage() {
        let path = "/tmp/test_kvdb.db";
        fresh_path(path);

        let mut db = KvDb::<i32, i32>::open(path);

        for i in 1..=12 {
            db.put(i, i * 10);
        }

        for i in 1..=12 {
            assert_eq!(db.get(&i), Some(i * 10), "get({i}) mismatch");
        }
        assert_eq!(db.get(&999), None, "missing key should return None");

        let range = db.range();
        let expected: Vec<(i32, i32)> = (1..=12).map(|i| (i, i * 10)).collect();
        assert_eq!(range, expected, "range() must return sorted (key, value) pairs");

        assert_eq!(db.len(), 12);

        let (found, value) = db.delete(12);
        assert!(found, "delete(12) should report found = true");
        assert_eq!(value, Some(120));
        assert_eq!(db.get(&12), None);
        assert_eq!(db.len(), 11);

        let (found, value) = db.delete(999);
        assert!(!found);
        assert_eq!(value, None);
        assert_eq!(db.len(), 11, "len unchanged after a failed delete");

        let (found, value) = db.delete(5);
        assert!(found);
        assert_eq!(value, Some(50));
        assert_eq!(db.get(&5), None);
        assert_eq!(db.len(), 10);

        fresh_path(path);
    }

    #[test]
    fn test_kvdb_string_keys_and_values() {
       
        let path = "/tmp/test_kvdb_strings.db";
        fresh_path(path);

        let mut db = KvDb::<String, String>::open(path);

        db.put("apple".to_string(), "fruit".to_string());
        db.put("carrot".to_string(), "vegetable".to_string());
        db.put("banana".to_string(), "fruit".to_string());

        assert_eq!(db.get(&"apple".to_string()), Some("fruit".to_string()));
        assert_eq!(db.get(&"banana".to_string()), Some("fruit".to_string()));
        assert_eq!(db.get(&"kiwi".to_string()), None);

        let range = db.range();
        assert_eq!(
            range,
            vec![
                ("apple".to_string(), "fruit".to_string()),
                ("banana".to_string(), "fruit".to_string()),
                ("carrot".to_string(), "vegetable".to_string()),
            ]
        );

        fresh_path(path);
    }
}
