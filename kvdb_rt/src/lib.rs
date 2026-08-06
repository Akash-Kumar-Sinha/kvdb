//! The async runtime glue behind `AsyncKvDb`: [`KvdbCall`], a hand-rolled
//! [`std::future::Future`] that dispatches a blocking call to a worker
//! thread, and [`ThreadPoolHandle`], the channel it dispatches through.
//!
//! Deliberately small and executor-free — see [`KvdbCall`] for why.

mod kvdb_call;
pub use kvdb_call::{KvdbCall, ThreadPoolHandle};
