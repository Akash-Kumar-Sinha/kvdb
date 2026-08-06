//! KvDB: a disk-backed, embedded key-value store — a B+tree storage engine
//! with a page-based on-disk format, a compile-time-enforced typestate API,
//! real thread-safe concurrency, zero-copy scanning, and a pluggable wire
//! format.
//!
//! [`KvDb`] is the type you actually use; see its docs for a complete
//! example. Everything else re-exported here — [`Value`], [`ValueError`],
//! [`DbError`], the [`Codec`] types, [`ScanIter`]/[`LendingIterator`] — is
//! what `KvDb`'s methods take or return. For an async handle that dispatches
//! calls to a worker pool instead of blocking, see the separate `async_kvdb`
//! crate's `AsyncKvDb`.
//!
//! The full design rationale — why a B+tree, why typestate, why `put`
//! accumulates, the concurrency model, the codec abstraction — lives in the
//! workspace README, not here; these doc comments describe *what* each item
//! does, not *why* the engine is shaped this way.

mod kvdb;

pub use btree::{DbError, Value, ValueError};
pub use codec::{BincodeCodec, Codec, CodecRegistry, Json, JsonCodec};
pub use kvdb::KvDb;
pub use scan::{LendingIterator, ScanIter};

#[cfg(test)]
mod tests {
    use super::*;
    use scan::LendingIterator;

    fn fresh_path(path: &str) {
        std::fs::remove_file(path).ok();
    }

    fn round_trip<R>(name: &str, value: impl Into<Value>) -> Result<R, DbError>
    where
        R: TryFrom<Value, Error = ValueError>,
    {
        let path = format!("/tmp/test_kvdb_{name}.db");
        fresh_path(&path);
        let mut db = KvDb::<i32>::open(&path)?;
        db.put(1, value)?;
        let stored = db.get::<R>(&1);
        fresh_path(&path);
        stored
    }

    fn scanned(db: &KvDb<i32>) -> Result<Vec<(i32, i32)>, DbError> {
        let mut iter = db.scan();
        let mut collected = Vec::new();
        while let Some(item) = iter.next() {
            let (key, value) = item?;
            collected.push((*key, i32::try_from(value.clone())?));
        }
        collected.sort_unstable_by_key(|(key, _)| *key);
        Ok(collected)
    }

    #[test]
    fn test_repeated_put_accumulates_into_one_key() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_multi_put.db";
        fresh_path(path);

        let mut db = KvDb::<i32>::open(path)?;
        db.put(5, 90)?;
        assert_eq!(db.get::<i32>(&5)?, 90, "a single put stays a plain value");

        db.put(5, 100)?;
        db.put(5, 8)?;
        db.put(5, 60)?;

        let all: Vec<Value> = db.get(&5)?;
        assert_eq!(
            all,
            vec![
                Value::I32(90),
                Value::I32(100),
                Value::I32(8),
                Value::I32(60)
            ],
            "every put must be readable, in the order it arrived"
        );

        assert_eq!(db.len()?, 1, "four puts of one key is still one entry");
        assert_eq!(db.range()?.len(), 1);

        assert!(
            db.get::<i32>(&5)
                .is_err_and(|err| matches!(err, DbError::Value(ValueError::TypeMismatch))),
            "an accumulated key is no longer a single i32, and must say so"
        );

        db.update(5, 7)?;
        assert_eq!(
            db.get::<i32>(&5)?,
            7,
            "update replaces the whole accumulator"
        );

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_accumulation_never_splices_a_stored_list() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_multi_vs_list.db";
        fresh_path(path);

        let mut db = KvDb::<i32>::open(path)?;
        let stored = Value::List(vec![Value::I32(1), Value::I32(2)]);
        db.put(1, stored.clone())?;
        db.put(1, 99i64)?;

        let all: Vec<Value> = db.get(&1)?;
        assert_eq!(
            all,
            vec![stored, Value::I64(99)],
            "a caller's own List must stay one element, not be spliced into the accumulator"
        );

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_accumulated_keys_survive_a_split() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_multi_split.db";
        fresh_path(path);

        let mut db = KvDb::<i32>::open(path)?;
        for i in 0..200 {
            db.put(i, i * 10)?;
        }
        for _ in 0..3 {
            for i in 0..200 {
                db.put(i, i)?;
            }
        }

        assert_eq!(db.len()?, 200, "repeated puts must not create new entries");
        for i in 0..200 {
            let all: Vec<Value> = db.get(&i)?;
            assert_eq!(
                all,
                vec![
                    Value::I32(i * 10),
                    Value::I32(i),
                    Value::I32(i),
                    Value::I32(i)
                ],
                "key {i} lost an accumulated value across splits"
            );
        }

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_kvdb_basic_usage() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_basic_usage.db";
        fresh_path(path);

        let mut db = KvDb::<i32>::open(path)?;

        for i in 1..=12 {
            db.put(i, i * 10)?;
        }

        for i in 1..=12 {
            assert_eq!(db.get::<i32>(&i)?, i * 10, "get({i}) mismatch");
        }

        assert!(
            db.get::<i32>(&999).is_err_and(|err| err.is_not_found()),
            "missing key should return NotFound"
        );

        let expected: Vec<(i32, Value)> = (1..=12).map(|i| (i, Value::I32(i * 10))).collect();
        assert_eq!(
            db.range()?,
            expected,
            "range() must return sorted (key, value) pairs"
        );

        assert_eq!(db.len()?, 12);

        let (found, value) = db.delete(12)?;
        assert!(found, "delete(12) should report found = true");
        assert_eq!(value, Some(Value::I32(120)));

        assert!(
            db.get::<i32>(&12).is_err_and(|err| err.is_not_found()),
            "12 should be gone after delete"
        );
        assert_eq!(db.len()?, 11);

        let (found, value) = db.delete(999)?;
        assert!(!found);
        assert_eq!(value, None);
        assert_eq!(db.len()?, 11, "len unchanged after a failed delete");

        let (found, value) = db.delete(5)?;
        assert!(found);
        assert_eq!(value, Some(Value::I32(50)));

        assert!(db.get::<i32>(&5).is_err_and(|err| err.is_not_found()));
        assert_eq!(db.len()?, 10);

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_kvdb_lock_unlock_round_trip() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_lock_unlock_round_trip.db";
        fresh_path(path);

        let mut db = KvDb::<i32>::open(path)?;
        db.put(1, 100)?;

        let mut db = db.lock();
        assert_eq!(db.get::<i32>(&1)?, 100, "get must still work while locked");

        let mut db = db.unlock();
        db.put(2, 200)?;
        assert_eq!(db.get::<i32>(&2)?, 200);

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_kvdb_string_keys_and_values() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_string_keys_and_values.db";
        fresh_path(path);

        let mut db = KvDb::<String>::open(path)?;

        db.put("apple".to_string(), "fruit".to_string())?;
        db.put("carrot".to_string(), "vegetable".to_string())?;
        db.put("banana".to_string(), "fruit".to_string())?;

        assert_eq!(db.get::<String>(&"apple".to_string())?, "fruit");
        assert_eq!(db.get::<String>(&"banana".to_string())?, "fruit");

        assert!(
            db.get::<String>(&"kiwi".to_string())
                .is_err_and(|err| err.is_not_found()),
            "missing key should return NotFound"
        );

        let expected: Vec<(String, Value)> = vec![
            ("apple".to_string(), Value::Text("fruit".to_string())),
            ("banana".to_string(), Value::Text("fruit".to_string())),
            ("carrot".to_string(), Value::Text("vegetable".to_string())),
        ];
        assert_eq!(db.range()?, expected);

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_kvdb_i8_value() -> Result<(), DbError> {
        assert_eq!(round_trip::<i8>("i8_value", 5i8)?, 5);
        Ok(())
    }

    #[test]
    fn test_kvdb_i32_value() -> Result<(), DbError> {
        assert_eq!(round_trip::<i32>("i32_value", 42i32)?, 42);
        Ok(())
    }

    #[test]
    fn test_kvdb_i64_value() -> Result<(), DbError> {
        assert_eq!(
            round_trip::<i64>("i64_value", 9_000_000_000i64)?,
            9_000_000_000
        );
        Ok(())
    }

    #[test]
    fn test_kvdb_u8_value() -> Result<(), DbError> {
        assert_eq!(round_trip::<u8>("u8_value", 200u8)?, 200);
        Ok(())
    }

    #[test]
    fn test_kvdb_u32_value() -> Result<(), DbError> {
        assert_eq!(
            round_trip::<u32>("u32_value", 4_000_000_000u32)?,
            4_000_000_000
        );
        Ok(())
    }

    #[test]
    fn test_kvdb_u64_value() -> Result<(), DbError> {
        assert_eq!(
            round_trip::<u64>("u64_value", 18_000_000_000_000_000_000u64)?,
            18_000_000_000_000_000_000
        );
        Ok(())
    }

    #[test]
    fn test_kvdb_f32_value() -> Result<(), DbError> {
        assert_eq!(round_trip::<f32>("f32_value", 3.5f32)?, 3.5);
        Ok(())
    }

    #[test]
    fn test_kvdb_f64_value() -> Result<(), DbError> {
        assert_eq!(
            round_trip::<f64>("f64_value", std::f64::consts::PI)?,
            std::f64::consts::PI
        );
        Ok(())
    }

    #[test]
    fn test_kvdb_char_value() -> Result<(), DbError> {
        assert_eq!(round_trip::<char>("char_value", 'R')?, 'R');
        Ok(())
    }

    #[test]
    fn test_kvdb_text_value() -> Result<(), DbError> {
        assert_eq!(
            round_trip::<String>("text_value", "hello world".to_string())?,
            "hello world"
        );
        Ok(())
    }

    #[test]
    fn test_kvdb_bytes_value() -> Result<(), DbError> {
        assert_eq!(
            round_trip::<Vec<u8>>("bytes_value", vec![1u8, 2, 3, 4, 5])?,
            vec![1, 2, 3, 4, 5]
        );
        Ok(())
    }

    #[test]
    fn test_kvdb_list_value() -> Result<(), DbError> {
        let list = Value::List(vec![Value::I32(1), Value::I32(2), Value::I32(3)]);
        assert_eq!(
            round_trip::<Vec<Value>>("list_value", list)?,
            vec![Value::I32(1), Value::I32(2), Value::I32(3)]
        );
        Ok(())
    }

    #[test]
    fn test_kvdb_pair_value() -> Result<(), DbError> {
        let left = vec![Value::I32(1), Value::I32(2)];
        let right = vec![Value::Text("a".to_string()), Value::Text("b".to_string())];
        let stored: (Vec<Value>, Vec<Value>) =
            round_trip("pair_value", (left.clone(), right.clone()))?;
        assert_eq!(stored, (left, right));
        Ok(())
    }

    #[test]
    fn test_kvdb_type_mismatch_is_not_not_found() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_type_mismatch.db";
        fresh_path(path);
        let mut db = KvDb::<i32>::open(path)?;
        db.put(1, 42i32)?;

        let err = db.get::<i64>(&1).expect_err("i32 is not readable as i64");
        assert!(!err.is_not_found(), "a mismatch is not a missing key");
        assert!(matches!(err, DbError::Value(ValueError::TypeMismatch)));

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_kvdb_scan() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_scan.db";
        fresh_path(path);

        let mut db = KvDb::<i32>::open(path)?;
        for i in 1..=8 {
            db.put(i, i * 10)?;
        }

        let expected: Vec<(i32, i32)> = (1..=8).map(|i| (i, i * 10)).collect();
        assert_eq!(
            scanned(&db)?,
            expected,
            "scan() must yield every inserted key/value exactly once"
        );

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_kvdb_runs_on_a_swapped_codec() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_json_codec.db";
        fresh_path(path);

        let codec = CodecRegistry::default()
            .create("json")
            .expect("json is built in");
        let mut db = KvDb::<i32>::open_with_codec(path, codec)?;

        for i in 1..=40 {
            db.put(i, i * 10)?;
        }
        db.put(
            41,
            Value::List(vec![Value::Char('π'), Value::F64(f64::NAN)]),
        )?;

        for i in 1..=40 {
            assert_eq!(db.get::<i32>(&i)?, i * 10, "get({i}) mismatch");
        }
        let nested: Vec<Value> = db.get(&41)?;
        assert!(matches!(nested[0], Value::Char('π')));
        assert!(
            matches!(nested[1], Value::F64(number) if number.is_nan()),
            "NaN must survive a format with no NaN literal"
        );

        assert_eq!(db.len()?, 41);
        assert_eq!(db.delete(20)?, (true, Some(Value::I32(200))));
        assert!(db.get::<i32>(&20).is_err_and(|err| err.is_not_found()));

        let mut scanned = 0;
        let mut iter = db.scan();
        while let Some(item) = iter.next() {
            item?;
            scanned += 1;
        }
        assert_eq!(scanned, 40, "scan() must see every remaining entry");

        let raw = std::fs::read(path).expect("the database file exists");
        assert!(
            String::from_utf8_lossy(&raw).contains(r#"{"i32":100}"#),
            "the file should hold readable json, not bincode"
        );

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_codecs_produce_different_files() -> Result<(), DbError> {
        let paths = [
            "/tmp/test_kvdb_fmt_bincode.db",
            "/tmp/test_kvdb_fmt_json.db",
        ];
        let codecs: [Box<dyn Codec>; 2] = [Box::new(BincodeCodec), Box::new(JsonCodec)];

        let mut files: Vec<Vec<u8>> = Vec::with_capacity(2);
        for (path, codec) in std::iter::zip(paths, codecs) {
            fresh_path(path);
            let mut db = KvDb::<i32>::open_with_codec(path, codec)?;
            db.put(1, "same input".to_string())?;
            files.push(std::fs::read(path).expect("page was written"));
        }
        let [bincode_file, json_file]: [Vec<u8>; 2] = files.try_into().expect("one file per codec");

        assert_ne!(bincode_file, json_file);
        assert!(
            String::from_utf8_lossy(&json_file).contains(r#"{"text":"same input"}"#),
            "the json file should spell out the value it stored"
        );
        assert!(
            !String::from_utf8_lossy(&bincode_file).contains(r#"{"text""#),
            "the bincode file should not"
        );
        for path in paths {
            fresh_path(path);
        }
        Ok(())
    }
}

#[cfg(test)]
mod concurrency {
    use super::*;
    use scan::LendingIterator;
    use std::thread::{self, JoinHandle};

    const WRITERS: i32 = 8;
    const PER_WRITER: i32 = 25;
    const TOTAL: i32 = WRITERS * PER_WRITER;

    fn key_for(writer: i32, i: i32) -> i32 {
        i * WRITERS + writer
    }

    fn fresh_path(path: &str) {
        std::fs::remove_file(path).ok();
    }

    fn join_all(handles: Vec<JoinHandle<Result<(), DbError>>>) -> Result<(), DbError> {
        let mut first_error = Ok(());
        for handle in handles {
            let result = handle.join().expect("worker thread panicked");
            if first_error.is_ok() {
                first_error = result;
            }
        }
        first_error
    }

    fn spawn_writers(db: &KvDb<i32>) -> Vec<JoinHandle<Result<(), DbError>>> {
        (0..WRITERS)
            .map(|writer| {
                let mut db = db.clone();
                thread::spawn(move || {
                    for i in 0..PER_WRITER {
                        let key = key_for(writer, i);
                        db.put(key, key * 10)?;
                    }
                    Ok(())
                })
            })
            .collect()
    }

    fn assert_intact(db: &mut KvDb<i32>) -> Result<(), DbError> {
        assert_eq!(db.len()?, TOTAL as usize, "a concurrent write was lost");

        for key in 0..TOTAL {
            assert_eq!(db.get::<i32>(&key)?, key * 10, "get({key}) came back wrong");
        }

        let keys: Vec<i32> = db.range()?.into_iter().map(|(key, _)| key).collect();
        let expected: Vec<i32> = (0..TOTAL).collect();
        assert_eq!(
            keys, expected,
            "range() must be sorted and complete after concurrent splits"
        );

        let mut iter = db.scan();
        let mut scanned = Vec::with_capacity(TOTAL as usize);
        while let Some(item) = iter.next() {
            let (key, value) = item?;
            scanned.push((*key, i32::try_from(value.clone())?));
        }
        scanned.sort_unstable_by_key(|(key, _)| *key);
        let expected: Vec<(i32, i32)> = (0..TOTAL).map(|key| (key, key * 10)).collect();
        assert_eq!(
            scanned, expected,
            "scan() must agree with range() after concurrent splits"
        );
        Ok(())
    }

    #[test]
    fn test_concurrent_reads_single_writer() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_concurrent.db";
        fresh_path(path);

        let mut db = KvDb::<i32>::open(path)?;
        for i in 1..=8 {
            db.put(i, i * 10)?;
        }

        let readers: Vec<_> = (0..4)
            .map(|_| {
                let mut db = db.clone();
                thread::spawn(move || {
                    for i in 1..=8 {
                        assert_eq!(db.get::<i32>(&i)?, i * 10, "get({i}) mismatch");
                    }
                    Ok(())
                })
            })
            .collect();

        join_all(readers)?;

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_concurrent_writers_do_not_lose_writes() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_multi_writer.db";
        fresh_path(path);

        let db = KvDb::<i32>::open(path)?;
        join_all(spawn_writers(&db))?;

        let mut db = db;
        assert_intact(&mut db)?;

        let pages = std::fs::metadata(path).expect("database file exists").len() / 4096;
        assert!(
            pages > 20,
            "expected splits to allocate many pages, saw {pages}"
        );

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_readers_see_a_consistent_tree_during_concurrent_writes() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_multi_writer_readers.db";
        fresh_path(path);

        let db = KvDb::<i32>::open(path)?;
        let mut handles = spawn_writers(&db);

        handles.extend((0..4).map(|_| {
            let mut db = db.clone();
            thread::spawn(move || {
                for _ in 0..8 {
                    for key in 0..TOTAL {
                        match db.get::<i32>(&key) {
                            Ok(value) => assert_eq!(value, key * 10, "torn value at {key}"),
                            Err(err) if err.is_not_found() => {}
                            Err(err) => return Err(err),
                        }
                    }

                    let keys: Vec<i32> = db.range()?.into_iter().map(|(key, _)| key).collect();
                    let mut sorted = keys.clone();
                    sorted.sort_unstable();
                    assert_eq!(keys, sorted, "range() saw an out-of-order tree");
                }
                Ok(())
            })
        }));

        join_all(handles)?;

        let mut db = db;
        assert_intact(&mut db)?;

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_concurrent_updates_insert_a_key_exactly_once() -> Result<(), DbError> {
        const ROUNDS: i32 = 25;

        let path = "/tmp/test_kvdb_multi_writer_update.db";
        fresh_path(path);

        let db = KvDb::<i32>::open(path)?;
        let mut counter = db.clone();

        for round in 0..ROUNDS {
            let writers: Vec<_> = (0..WRITERS)
                .map(|writer| {
                    let mut db = db.clone();
                    thread::spawn(move || {
                        db.update(round, writer)?;
                        Ok(())
                    })
                })
                .collect();

            join_all(writers)?;

            assert_eq!(
                counter.len()?,
                (round + 1) as usize,
                "round {round}: racing update() calls inserted the key twice"
            );
            let winner = counter.get::<i32>(&round)?;
            assert!(
                (0..WRITERS).contains(&winner),
                "round {round}: stored value {winner} was written by no thread"
            );
        }

        fresh_path(path);
        Ok(())
    }

    #[test]
    fn test_concurrent_deletes_keep_the_survivors() -> Result<(), DbError> {
        let path = "/tmp/test_kvdb_multi_writer_delete.db";
        fresh_path(path);

        let db = KvDb::<i32>::open(path)?;
        join_all(spawn_writers(&db))?;

        let deleters: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let mut db = db.clone();
                thread::spawn(move || {
                    for i in (0..PER_WRITER).step_by(2) {
                        let key = key_for(writer, i);
                        let (found, value) = db.delete(key)?;
                        assert!(found, "delete({key}) reported not found");
                        assert_eq!(value, Some(Value::I32(key * 10)));
                    }
                    Ok(())
                })
            })
            .collect();

        join_all(deleters)?;

        let mut db = db;
        let survivors: Vec<i32> = (0..TOTAL)
            .filter(|key| ((key / WRITERS) % 2) == 1)
            .collect();
        assert_eq!(
            db.len()?,
            survivors.len(),
            "concurrent deletes lost or kept the wrong entries"
        );

        let keys: Vec<i32> = db.range()?.into_iter().map(|(key, _)| key).collect();
        assert_eq!(keys, survivors, "range() must hold exactly the survivors");

        for key in &survivors {
            assert_eq!(
                db.get::<i32>(key)?,
                key * 10,
                "survivor {key} was corrupted"
            );
        }
        for key in (0..TOTAL).filter(|key| ((key / WRITERS) % 2) == 0) {
            assert!(
                db.get::<i32>(&key).is_err_and(|err| err.is_not_found()),
                "deleted key {key} is still readable"
            );
        }

        fresh_path(path);
        Ok(())
    }
}
