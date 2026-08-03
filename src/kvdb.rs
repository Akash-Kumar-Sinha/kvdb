use btree::{BTree, Initialized, Locked, Uninitialized, Unlocked, Value, ValueError};
use serde::{Serialize, de::DeserializeOwned};

pub struct KvDb<S, LockState = Unlocked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    inner: BTree<S, Initialized, LockState>,
}

impl<S> KvDb<S, Unlocked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    pub fn open(path: &str) -> Self {
        let tree = BTree::<S, Uninitialized, Locked>::new(path);
        KvDb {
            inner: tree.unlock(),
        }
    }

    pub fn put(&mut self, key: S, value: impl Into<btree::Value>) {
        self.inner.put(key, value);
    }

    pub fn delete(&mut self, key: S) -> (bool, Option<Value>) {
        self.inner.delete(key)
    }

    pub fn lock(self) -> KvDb<S, Locked> {
        KvDb {
            inner: self.inner.lock(),
        }
    }
}

impl<S> KvDb<S, Locked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    pub fn unlock(self) -> KvDb<S, Unlocked> {
        KvDb {
            inner: self.inner.unlock(),
        }
    }
}

impl<S, LockState> KvDb<S, LockState>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    pub fn get<R>(&mut self, key: &S) -> Result<R, ValueError>
    where
        R: TryFrom<Value, Error = ValueError>,
    {
        self.inner.get(key)
    }

    pub fn range(&mut self) -> Vec<(S, Value)> {
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

        let mut db = KvDb::<i32>::open(path);

        for i in 1..=12 {
            db.put(i, i * 10);
        }

        for i in 1..=12 {
            let value: i32 = db.get(&i).expect("get failed");
            assert_eq!(value, i * 10, "get({i}) mismatch");
        }

        assert!(
            matches!(db.get::<i32>(&999), Err(ValueError::NotFound)),
            "missing key should return NotFound"
        );

        let range = db.range();
        let expected: Vec<(i32, Value)> = (1..=12).map(|i| (i, Value::I32(i * 10))).collect();
        assert_eq!(
            range, expected,
            "range() must return sorted (key, value) pairs"
        );

        assert_eq!(db.len(), 12);

        let (found, value) = db.delete(12);
        assert!(found, "delete(12) should report found = true");
        let value: i32 = value
            .expect("value missing")
            .try_into()
            .expect("expected int");
        assert_eq!(value, 120);

        assert!(
            matches!(db.get::<i32>(&12), Err(ValueError::NotFound)),
            "12 should be gone after delete"
        );
        assert_eq!(db.len(), 11);

        let (found, value) = db.delete(999);
        assert!(!found);
        assert_eq!(value, None);
        assert_eq!(db.len(), 11, "len unchanged after a failed delete");

        let (found, value) = db.delete(5);
        assert!(found);
        let value: i32 = value
            .expect("value missing")
            .try_into()
            .expect("expected int");
        assert_eq!(value, 50);

        assert!(matches!(db.get::<i32>(&5), Err(ValueError::NotFound)));
        assert_eq!(db.len(), 10);

        fresh_path(path);
    }

    #[test]
    fn test_kvdb_lock_unlock_round_trip() {
        let path = "/tmp/test_kvdb_lock.db";
        fresh_path(path);

        let mut db = KvDb::<i32>::open(path);
        db.put(1, 100);

        let mut db = db.lock();
        let value: i32 = db.get(&1).expect("get must still work while locked");
        assert_eq!(value, 100);

        let mut db = db.unlock();
        db.put(2, 200);
        let value: i32 = db.get(&2).expect("get failed");
        assert_eq!(value, 200);

        fresh_path(path);
    }

    #[test]
    fn test_kvdb_string_keys_and_values() {
        let path = "/tmp/test_kvdb_strings.db";
        fresh_path(path);

        let mut db = KvDb::<String>::open(path);

        db.put("apple".to_string(), "fruit".to_string());
        db.put("carrot".to_string(), "vegetable".to_string());
        db.put("banana".to_string(), "fruit".to_string());

        let value: String = db.get(&"apple".to_string()).expect("get failed");
        assert_eq!(value, "fruit");

        let value: String = db.get(&"banana".to_string()).expect("get failed");
        assert_eq!(value, "fruit");

        assert!(
            matches!(
                db.get::<String>(&"kiwi".to_string()),
                Err(ValueError::NotFound)
            ),
            "missing key should return NotFound"
        );

        let range = db.range();
        let expected: Vec<(String, Value)> = vec![
            ("apple".to_string(), Value::Text("fruit".to_string())),
            ("banana".to_string(), Value::Text("fruit".to_string())),
            ("carrot".to_string(), Value::Text("vegetable".to_string())),
        ];
        assert_eq!(range, expected);

        fresh_path(path);
    }
}
