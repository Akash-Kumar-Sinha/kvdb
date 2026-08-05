mod async_kvdb;
mod async_scan;

pub use async_kvdb::AsyncKvDb;
pub use async_scan::{AsyncScanIter, NextCall};
pub use btree::{DbError, Value, ValueError};

#[cfg(test)]
mod tests {
    mod async_tests {
        use crate::async_kvdb::AsyncKvDb;
        use btree::{DbError, Value};
        use std::future::Future;
        use std::pin::Pin;
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

        fn dummy_waker() -> Waker {
            fn no_op(_: *const ()) {}
            fn clone(_: *const ()) -> RawWaker {
                raw()
            }
            fn raw() -> RawWaker {
                static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
                RawWaker::new(std::ptr::null(), &VTABLE)
            }
            unsafe { Waker::from_raw(raw()) }
        }

        fn block_on<F: Future>(mut future: F) -> F::Output {
            let waker = dummy_waker();
            let mut cx = Context::from_waker(&waker);
            let mut future = unsafe { Pin::new_unchecked(&mut future) };
            loop {
                match future.as_mut().poll(&mut cx) {
                    Poll::Ready(val) => return val,
                    Poll::Pending => std::thread::yield_now(),
                }
            }
        }

        fn fresh_path(path: &str) {
            std::fs::remove_file(path).ok();
        }

        #[test]
        fn test_async_kvdb_basic() -> Result<(), DbError> {
            let path = "/tmp/test_async_kvdb_basic.db";
            fresh_path(path);

            let result = block_on(async {
                let db = AsyncKvDb::<i32>::open(path, 4)?;

                for i in 1..=8 {
                    db.put(i, i * 10).await?;
                }

                for i in 1..=8 {
                    assert_eq!(db.get::<i32>(i).await?, i * 10, "get({i}) mismatch");
                }

                assert!(
                    db.get::<i32>(999)
                        .await
                        .is_err_and(|err| err.is_not_found())
                );

                assert_eq!(db.len().await?, 8);

                let expected: Vec<(i32, Value)> =
                    (1..=8).map(|i| (i, Value::I32(i * 10))).collect();
                assert_eq!(
                    db.range().await?,
                    expected,
                    "range() must return sorted pairs"
                );

                let (found, value) = db.delete(5).await?;
                assert!(found, "delete(5) should report found = true");
                assert_eq!(value, Some(Value::I32(50)));

                assert!(db.get::<i32>(5).await.is_err_and(|err| err.is_not_found()));
                assert_eq!(db.len().await?, 7, "len should reflect the delete");
                Ok(())
            });

            fresh_path(path);
            result
        }

        #[test]
        fn test_async_kvdb_lock_unlock() -> Result<(), DbError> {
            let path = "/tmp/test_async_kvdb_lock.db";
            fresh_path(path);

            let result = block_on(async {
                let db = AsyncKvDb::<i32>::open(path, 2)?;
                db.put(1, 100).await?;

                let db = db.lock();
                assert_eq!(
                    db.get::<i32>(1).await?,
                    100,
                    "get must still work while locked"
                );

                let db = db.unlock();
                db.put(2, 200).await?;
                assert_eq!(db.get::<i32>(2).await?, 200);
                Ok(())
            });

            fresh_path(path);
            result
        }

        #[test]
        fn test_async_kvdb_scan() -> Result<(), DbError> {
            let path = "/tmp/test_async_kvdb_scan.db";
            fresh_path(path);

            let result = block_on(async {
                let db = AsyncKvDb::<i32>::open(path, 4)?;
                for i in 1..=8 {
                    db.put(i, i * 10).await?;
                }

                let mut iter = db.scan();
                let mut collected: Vec<(i32, i32)> = Vec::new();
                while let Some(item) = iter.next().await {
                    let (key, value) = item?;
                    collected.push((key, i32::try_from(value)?));
                }
                collected.sort_by_key(|(k, _)| *k);

                let expected: Vec<(i32, i32)> = (1..=8).map(|i| (i, i * 10)).collect();
                assert_eq!(
                    collected, expected,
                    "async scan() must yield every inserted key/value in order"
                );
                Ok(())
            });

            fresh_path(path);
            result
        }

        #[test]
        fn test_async_kvdb_concurrent_writers() -> Result<(), DbError> {
            let path = "/tmp/test_async_kvdb_concurrent.db";
            fresh_path(path);

            let result = block_on(async {
                let db = AsyncKvDb::<i32>::open(path, 4)?;

                let writes: Vec<_> = (0..60).map(|key| db.put(key, key * 10)).collect();
                for write in writes {
                    write.await?;
                }

                assert_eq!(db.len().await?, 60, "a concurrent write was lost");
                for key in 0..60 {
                    assert_eq!(db.get::<i32>(key).await?, key * 10);
                }
                Ok(())
            });

            fresh_path(path);
            result
        }
    }
}
