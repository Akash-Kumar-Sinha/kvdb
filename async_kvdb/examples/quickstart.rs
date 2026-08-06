use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use async_kvdb::{AsyncKvDb, DbError, Value};

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
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn main() -> Result<(), DbError> {
    let path = "/tmp/kvdb_async_quickstart.db";
    std::fs::remove_file(path).ok();

    let db = AsyncKvDb::<u32>::open(path, 4)?;

    block_on(async {
        db.put(1, "draft".to_string()).await?;
        db.put(1, "published".to_string()).await?;

        let history: Vec<Value> = db.get(1).await?;
        println!("doc 1 history   -> {history:?}");

        db.update(2, 42i32).await?;
        println!("doc 2           -> {}", db.get::<i32>(2).await?);
        println!("len             -> {}", db.len().await?);

        let mut iter = db.scan();
        while let Some(item) = iter.next().await {
            let (key, value) = item?;
            println!("scan            -> {key}: {value:?}");
        }

        let (found, previous) = db.delete(2).await?;
        println!("delete(2)       -> found={found}, previous={previous:?}");

        Ok::<(), DbError>(())
    })?;

    let pending: Vec<_> = (10..14).map(|key| db.put(key, key as i32)).collect();
    block_on(async {
        for call in pending {
            call.await?;
        }
        println!("after 4 dispatched puts -> len {}", db.len().await?);
        Ok::<(), DbError>(())
    })?;

    std::fs::remove_file(path).ok();
    Ok(())
}
