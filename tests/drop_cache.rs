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

//! Dropping a `Cache` stops its watcher, also while the watcher establishes the cache: while the
//! source is unreachable, while it does not support subscribing, or while the initial flush does
//! not finish.
//!
//! The watcher owns the source and the stream it subscribed, and drops them when it stops. The
//! test source puts a sender of `steps` in both, so `steps` closes when the watcher stops.

use std::time::Duration;

use futures::stream;
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
use tokio::sync::mpsc;

type Item = Result<Event<Val>, ConnectionClosed>;

/// How the remote data store answers a subscription.
#[derive(Clone, Copy)]
enum Remote {
    /// The connection fails: the watcher retries, with a backoff.
    Unreachable,
    /// Subscribing is unsupported: the watcher subscribes again 5 minutes later.
    Unsupported,
    /// The initial flush sends one key, then nothing more.
    StalledFlush,
}

/// A source that sends each step of the watcher to `steps`: `"subscribe"` for a subscription,
/// and `"flush"` when the watcher waits for the rest of a stalled initial flush.
struct ProbeSource {
    remote: Remote,
    steps: mpsc::UnboundedSender<&'static str>,
}

#[async_trait::async_trait]
impl Source<Val> for ProbeSource {
    async fn subscribe(
        &self,
        _left: &str,
        _right: &str,
    ) -> Result<EventStream<Val>, SubscribeError> {
        self.steps.send("subscribe").ok();

        match self.remote {
            Remote::Unreachable => Err(ConnectionClosed::new_str("unreachable").into()),
            Remote::Unsupported => Err(Unsupported::new("no subscription").into()),
            Remote::StalledFlush => {
                let first: Item = Ok(Event::Initialization(Change::new(
                    "dir/k1",
                    None,
                    Some(Val::new(1, "a")),
                )));
                let steps = self.steps.clone();
                let rest = stream::once(async move {
                    steps.send("flush").ok();
                    std::future::pending::<Item>().await
                });
                Ok(stream::iter([first]).chain(rest).boxed())
            }
        }
    }
}

#[derive(Debug, Default)]
struct ProbeConfig;

impl TypeConfig for ProbeConfig {
    type Value = Val;
    type Source = ProbeSource;

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

/// A cache on `remote`, and the steps of its watcher.
async fn new_cache(remote: Remote) -> (Cache<ProbeConfig>, mpsc::UnboundedReceiver<&'static str>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let source = ProbeSource { remote, steps: tx };
    let cache = Cache::<ProbeConfig>::new(source, "dir", "drop-cache").await;
    (cache, rx)
}

/// Drop `cache`, and assert that its watcher stops: `steps` closes.
async fn assert_drop_stops_watcher(
    cache: Cache<ProbeConfig>,
    mut steps: mpsc::UnboundedReceiver<&'static str>,
) {
    drop(cache);

    let stopped = async { while steps.recv().await.is_some() {} };
    tokio::time::timeout(Duration::from_secs(5), stopped)
        .await
        .expect("the watcher must stop when its cache is dropped");
}

/// The source is unreachable: the watcher retries to subscribe.
#[tokio::test]
async fn test_drop_while_source_unreachable() {
    let (cache, mut steps) = new_cache(Remote::Unreachable).await;

    // The first subscription fails, and the watcher retries.
    assert_eq!(steps.recv().await, Some("subscribe"));
    assert_eq!(steps.recv().await, Some("subscribe"));

    assert_drop_stops_watcher(cache, steps).await;
}

/// The source does not support subscribing: the watcher marks the cache unsupported, then waits
/// 5 minutes to subscribe again.
#[tokio::test]
async fn test_drop_while_source_unsupported() {
    let (cache, steps) = new_cache(Remote::Unsupported).await;

    // The read waits until the watcher has marked the cache.
    let got = cache.try_get("dir/k1").await.unwrap_err();
    assert_eq!(got.to_string(), "Unsupported: no subscription");

    assert_drop_stops_watcher(cache, steps).await;
}

/// The initial flush stalls: the watcher waits for the rest of it.
#[tokio::test]
async fn test_drop_while_initial_flush_stalls() {
    let (cache, mut steps) = new_cache(Remote::StalledFlush).await;

    assert_eq!(steps.recv().await, Some("subscribe"));
    assert_eq!(steps.recv().await, Some("flush"));

    assert_drop_stops_watcher(cache, steps).await;
}
