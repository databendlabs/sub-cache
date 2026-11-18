// Copyright 2021 Datafuse Labs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! A failed initial flush is never served as a ready cache.
//!
//! A reader that queues while the watcher fetches an initial flush runs when the flush fails part
//! way, before the retry. It gets the data from before the flush, a complete copy or an error,
//! never the part of the flush that arrived.

use std::collections::VecDeque;
use std::pin::pin;
use std::sync::Arc;
use std::sync::Mutex;

use futures::future::BoxFuture;
use futures::poll;
use futures::stream;
use futures::FutureExt;
use futures::StreamExt;
use sub_cache::errors::ConnectionClosed;
use sub_cache::errors::SubscribeError;
use sub_cache::errors::Unsupported;
use sub_cache::event_stream::Change;
use sub_cache::event_stream::Event;
use sub_cache::event_stream::EventStream;
use sub_cache::testing::source::Val;
use sub_cache::Cache;
use sub_cache::Source;
use sub_cache::TypeConfig;
use tokio::sync::oneshot;

type Item = Result<Event<Val>, ConnectionClosed>;

/// A source that serves the n-th subscription with the stream of the n-th future in `streams`;
/// a subscription beyond them never returns.
#[derive(Clone)]
struct ScriptedSource {
    streams: Arc<Mutex<VecDeque<BoxFuture<'static, EventStream<Val>>>>>,
}

impl ScriptedSource {
    fn new(streams: Vec<BoxFuture<'static, EventStream<Val>>>) -> Self {
        Self {
            streams: Arc::new(Mutex::new(streams.into())),
        }
    }
}

#[async_trait::async_trait]
impl Source<Val> for ScriptedSource {
    async fn subscribe(
        &self,
        _left: &str,
        _right: &str,
    ) -> Result<EventStream<Val>, SubscribeError> {
        let next = self.streams.lock().unwrap().pop_front();
        match next {
            Some(stream) => Ok(stream.await),
            None => std::future::pending().await,
        }
    }
}

#[derive(Debug, Default)]
struct ScriptedConfig;

impl TypeConfig for ScriptedConfig {
    type Value = Val;
    type Source = ScriptedSource;

    fn value_seq(value: &Self::Value) -> u64 {
        value.seq
    }

    fn spawn<F>(future: F, _name: impl ToString)
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::spawn(future);
    }
}

/// An event of an initial flush: `key` is `data` at `seq`.
fn init(key: &str, seq: u64, data: &str) -> Item {
    Ok(Event::Initialization(Change::new(
        key,
        None,
        Some(Val::new(seq, data)),
    )))
}

fn complete() -> Item {
    Ok(Event::InitializationComplete)
}

fn broken() -> Item {
    Err(ConnectionClosed::new_str(
        "connection reset during the initial flush",
    ))
}

/// A connection that sends `items`, then stays open.
fn connection(items: Vec<Item>) -> EventStream<Val> {
    stream::iter(items).chain(stream::pending()).boxed()
}

fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// All entries of `cache` as `(key, data)`; waits while the watcher holds the data.
async fn entries(cache: &Cache<ScriptedConfig>) -> Result<Vec<(String, String)>, Unsupported> {
    cache
        .try_access(|cache_data| {
            let data = &cache_data.data;
            data.iter()
                .map(|(k, v)| (k.clone(), v.data.clone()))
                .collect()
        })
        .await
}

/// The first initial flush breaks part way: a reader queued on it gets an error, and a later
/// reader gets the complete flush of the retry.
#[tokio::test]
async fn test_failed_first_initial_flush_is_not_served() {
    let (gate_tx, gate_rx) = oneshot::channel::<()>();

    let source = ScriptedSource::new(vec![
        async move {
            gate_rx.await.ok();
            connection(vec![init("dir/k1", 1, "a"), broken()])
        }
        .boxed(),
        async move {
            connection(vec![
                init("dir/k1", 1, "a"),
                init("dir/k2", 2, "b"),
                complete(),
            ])
        }
        .boxed(),
    ]);

    // `new()` returns when the watcher holds the data to fetch the first flush, which waits for
    // `gate`.
    let cache = Cache::<ScriptedConfig>::new(source, "dir", "first-flush").await;

    let mut read = pin!(entries(&cache));
    assert!(
        poll!(read.as_mut()).is_pending(),
        "the read must queue on the data"
    );

    // The flush breaks after `k1`; the queued read runs before the retry.
    gate_tx.send(()).unwrap();
    let got = read.await;
    assert!(
        got.is_err(),
        "a reader got a part of the initial flush: {got:?}"
    );

    let got = entries(&cache).await.unwrap();
    assert_eq!(got, kv(&[("dir/k1", "a"), ("dir/k2", "b")]));
}

/// The initial flush of a reconnect breaks part way: a reader queued on it gets the copy from
/// before the reconnect, which is complete.
#[tokio::test]
async fn test_failed_reconnect_initial_flush_is_not_served() {
    let (break_tx, break_rx) = oneshot::channel::<()>();
    let (subscribed_tx, subscribed_rx) = oneshot::channel::<()>();
    let (gate_tx, gate_rx) = oneshot::channel::<()>();

    let source = ScriptedSource::new(vec![
        async move {
            let flush = stream::iter(vec![
                init("dir/k1", 1, "a"),
                init("dir/k2", 2, "b"),
                complete(),
            ]);
            let breaks = stream::once(async move {
                break_rx.await.ok();
                broken()
            });
            flush.chain(breaks).boxed()
        }
        .boxed(),
        async move {
            // The watcher holds the data while it subscribes.
            subscribed_tx.send(()).ok();
            gate_rx.await.ok();
            connection(vec![init("dir/k1", 3, "a2"), broken()])
        }
        .boxed(),
    ]);

    let cache = Cache::<ScriptedConfig>::new(source, "dir", "reconnect-flush").await;
    let before = kv(&[("dir/k1", "a"), ("dir/k2", "b")]);
    assert_eq!(entries(&cache).await.unwrap(), before);

    // The connection breaks, and the watcher subscribes again.
    break_tx.send(()).unwrap();
    subscribed_rx.await.unwrap();

    let mut read = pin!(entries(&cache));
    assert!(
        poll!(read.as_mut()).is_pending(),
        "the read must queue on the data"
    );

    // The flush of the reconnect breaks after `k1`; the queued read runs before the retry.
    gate_tx.send(()).unwrap();
    let got = read.await.unwrap();
    assert_eq!(got, before, "a reader got a part of the initial flush");
}
