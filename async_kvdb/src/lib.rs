mod async_kvdb;
mod async_scan;

pub use async_kvdb::AsyncKvDb;
pub use async_scan::{AsyncScanIter, NextCall};
pub use btree::{Value, ValueError};

#[cfg(test)]
mod tests {
    mod async_tests {
        use crate::async_kvdb::AsyncKvDb;
        use btree::{Value, ValueError};
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
        fn test_async_kvdb_basic() {
            let path = "/tmp/test_async_kvdb_basic.db";
            fresh_path(path);

            block_on(async {
                let db = AsyncKvDb::<i32>::open(path, 4);

                for i in 1..=8 {
                    db.put(i, i * 10).await;
                }

                for i in 1..=8 {
                    let value: i32 = db.get(i).await.expect("get failed");
                    assert_eq!(value, i * 10, "get({i}) mismatch");
                }

                assert!(matches!(
                    db.get::<i32>(999).await,
                    Err(ValueError::NotFound)
                ));

                let len = db.len().await;
                assert_eq!(len, 8);

                let range = db.range().await;
                let expected: Vec<(i32, Value)> =
                    (1..=8).map(|i| (i, Value::I32(i * 10))).collect();
                assert_eq!(range, expected, "range() must return sorted pairs");

                let (found, value) = db.delete(5).await;
                assert!(found, "delete(5) should report found = true");
                let value: i32 = value
                    .expect("value missing")
                    .try_into()
                    .expect("expected int");
                assert_eq!(value, 50);

                assert!(matches!(db.get::<i32>(5).await, Err(ValueError::NotFound)));
                assert_eq!(db.len().await, 7, "len should reflect the delete");
            });

            fresh_path(path);
        }

        #[test]
        fn test_async_kvdb_lock_unlock() {
            let path = "/tmp/test_async_kvdb_lock.db";
            fresh_path(path);

            block_on(async {
                let db = AsyncKvDb::<i32>::open(path, 2);
                db.put(1, 100).await;

                let db = db.lock();
                let value: i32 = db.get(1).await.expect("get must still work while locked");
                assert_eq!(value, 100);
                // db.put(2, 200); // would not compile — put only exists on Unlocked

                let db = db.unlock();
                db.put(2, 200).await;
                let value: i32 = db.get(2).await.expect("get failed");
                assert_eq!(value, 200);
            });

            fresh_path(path);
        }

        #[test]
        fn test_async_kvdb_scan() {
            let path = "/tmp/test_async_kvdb_scan.db";
            fresh_path(path);

            block_on(async {
                let db = AsyncKvDb::<i32>::open(path, 4);
                for i in 1..=8 {
                    db.put(i, i * 10).await;
                }

                let mut iter = db.scan();
                let mut collected: Vec<(i32, i32)> = Vec::new();
                while let Some((k, v)) = iter.next().await {
                    let value: i32 = i32::try_from(v).expect("expected int");
                    collected.push((k, value));
                }
                collected.sort_by_key(|(k, _)| *k);

                let expected: Vec<(i32, i32)> = (1..=8).map(|i| (i, i * 10)).collect();
                assert_eq!(
                    collected, expected,
                    "async scan() must yield every inserted key/value in order"
                );
            });

            fresh_path(path);
        }
    }
}
