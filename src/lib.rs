mod kvdb;

pub use btree::{Value, ValueError};
pub use kvdb::KvDb;
pub use scan::{LendingIterator, ScanIter};

#[cfg(test)]
mod tests {
    use super::*;
    use scan::LendingIterator;
    use std::thread;

    fn fresh_path(path: &str) {
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_kvdb_basic_usage() {
        let path = "/tmp/test_kvdb_basic_usage.db";
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
        let path = "/tmp/test_kvdb_lock_unlock_round_trip.db";
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
        let path = "/tmp/test_kvdb_string_keys_and_values.db";
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

    #[test]
    fn test_kvdb_i8_value() {
        let path = "/tmp/test_kvdb_i8_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        db.put(1, 5i8);
        let value: i8 = db.get(&1).expect("get failed");
        assert_eq!(value, 5);
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_i32_value() {
        let path = "/tmp/test_kvdb_i32_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        db.put(1, 42i32);
        let value: i32 = db.get(&1).expect("get failed");
        assert_eq!(value, 42);
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_i64_value() {
        let path = "/tmp/test_kvdb_i64_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        db.put(1, 9_000_000_000i64);
        let value: i64 = db.get(&1).expect("get failed");
        assert_eq!(value, 9_000_000_000);
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_u8_value() {
        let path = "/tmp/test_kvdb_u8_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        db.put(1, 200u8);
        let value: u8 = db.get(&1).expect("get failed");
        assert_eq!(value, 200);
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_u32_value() {
        let path = "/tmp/test_kvdb_u32_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        db.put(1, 4_000_000_000u32);
        let value: u32 = db.get(&1).expect("get failed");
        assert_eq!(value, 4_000_000_000);
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_u64_value() {
        let path = "/tmp/test_kvdb_u64_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        db.put(1, 18_000_000_000_000_000_000u64);
        let value: u64 = db.get(&1).expect("get failed");
        assert_eq!(value, 18_000_000_000_000_000_000);
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_f32_value() {
        let path = "/tmp/test_kvdb_f32_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        db.put(1, 3.5f32);
        let value: f32 = db.get(&1).expect("get failed");
        assert_eq!(value, 3.5);
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_f64_value() {
        let path = "/tmp/test_kvdb_f64_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        db.put(1, std::f64::consts::PI);
        let value: f64 = db.get(&1).expect("get failed");
        assert_eq!(value, std::f64::consts::PI);
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_char_value() {
        let path = "/tmp/test_kvdb_char_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        db.put(1, 'R');
        let value: char = db.get(&1).expect("get failed");
        assert_eq!(value, 'R');
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_text_value() {
        let path = "/tmp/test_kvdb_text_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        db.put(1, "hello world".to_string());
        let value: String = db.get(&1).expect("get failed");
        assert_eq!(value, "hello world");
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_bytes_value() {
        let path = "/tmp/test_kvdb_bytes_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        db.put(1, vec![1u8, 2, 3, 4, 5]);
        let value: Vec<u8> = db.get(&1).expect("get failed");
        assert_eq!(value, vec![1, 2, 3, 4, 5]);
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_list_value() {
        let path = "/tmp/test_kvdb_list_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        let list = Value::List(vec![Value::I32(1), Value::I32(2), Value::I32(3)]);
        db.put(1, list);
        let value: Vec<Value> = db.get(&1).expect("get failed");
        assert_eq!(value, vec![Value::I32(1), Value::I32(2), Value::I32(3)]);
        fresh_path(path);
    }

    #[test]
    fn test_kvdb_pair_value() {
        let path = "/tmp/test_kvdb_pair_value.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path);
        let left = vec![Value::I32(1), Value::I32(2)];
        let right = vec![Value::Text("a".to_string()), Value::Text("b".to_string())];
        db.put(1, (left.clone(), right.clone()));
        let value: (Vec<Value>, Vec<Value>) = db.get(&1).expect("get failed");
        assert_eq!(value, (left, right));
        fresh_path(path);
    }

    #[test]
    fn test_concurrent_reads_single_writer() {
        let path = "/tmp/test_kvdb_concurrent.db";
        fresh_path(path);

        let mut db = KvDb::<i32>::open(path);
        for i in 1..=8 {
            db.put(i, i * 10);
        }

        let handles: Vec<_> = (0..4)
            .map(|t| {
                let mut db = db.clone();
                thread::spawn(move || {
                    for i in 1..=8 {
                        let value: i32 = db.get(&i).expect("get failed");
                        assert_eq!(value, i * 10, "thread {t}: get({i}) mismatch");
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread panicked");
        }

        fresh_path(path);
    }

    #[test]
    fn test_kvdb_scan() {
        let path = "/tmp/test_kvdb_scan.db";
        fresh_path(path);

        let mut db = KvDb::<i32>::open(path);
        for i in 1..=8 {
            db.put(i, i * 10);
        }

        let mut iter = db.scan();
        let mut collected: Vec<(i32, i32)> = Vec::new();
        while let Some((k, v)) = iter.next() {
            let value: i32 = i32::try_from(v.clone()).expect("expected int");
            collected.push((*k, value));
        }
        collected.sort_by_key(|(k, _)| *k);

        let expected: Vec<(i32, i32)> = (1..=8).map(|i| (i, i * 10)).collect();
        assert_eq!(
            collected, expected,
            "scan() must yield every inserted key/value"
        );
        assert_eq!(
            collected.len(),
            8,
            "scan() must not skip or duplicate entries"
        );

        fresh_path(path);
    }
}