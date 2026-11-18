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

//! The non-blocking `&self` reads: `try_get_now`, `try_list_dir_now` and `try_access_now`.
//!
//! They answer at once from a ready cache, to any number of concurrent readers sharing it, and
//! return `None` while the cache cannot answer, without waiting for it.

use std::sync::Arc;
use std::time::Duration;

use futures::stream;
use futures::FutureExt;
use futures::StreamExt;
use sub_cache::errors::SubscribeError;
use sub_cache::event_stream::EventStream;
use sub_cache::testing::source::TestSource;
use sub_cache::testing::source::Val;
use sub_cache::testing::types::TestConfig;
use sub_cache::Cache;
use sub_cache::Source;
use sub_cache::TypeConfig;
use tokio::time::sleep;

/// Wait until the cache answers `key` with `data` without waiting.
async fn wait_for_now(cache: &Cache<TestConfig>, key: &str, data: &str) {
    for _ in 0..100 {
        if let Some(Ok(Some(v))) = cache.try_get_now(key) {
            if v.data == data {
                return;
            }
        }
        sleep(Duration::from_millis(20)).await;
    }
    panic!("the cache did not answer {key} = {data} without waiting");
}

#[tokio::test]
async fn test_read_now_from_a_ready_cache() {
    let source = TestSource::new();
    source.set("now/a", Some("a")).await;
    source.set("now/d/x", Some("x")).await;
    source.set("other/b", Some("b")).await;

    let cache = Arc::new(Cache::<TestConfig>::new(source.clone(), "now", "read-now").await);
    wait_for_now(&cache, "now/a", "a").await;

    // Readers share the cache without an outer lock.
    let readers = (0..8).map(|_| {
        let cache = cache.clone();
        tokio::spawn(async move {
            let a = cache.try_get_now("now/a").unwrap().unwrap().map(|v| v.data);
            let missing = cache.try_get_now("now/missing").unwrap().unwrap();
            let dir = cache.try_list_dir_now("now/d").unwrap().unwrap();
            let dir = dir
                .into_iter()
                .map(|(k, v)| (k, v.data))
                .collect::<Vec<_>>();
            (a, missing, dir)
        })
    });
    for reader in readers {
        let (a, missing, dir) = reader.await.unwrap();
        assert_eq!(a.as_deref(), Some("a"));
        assert_eq!(missing, None);
        assert_eq!(dir, vec![("now/d/x".to_string(), "x".to_string())]);
    }

    // Changes reach the reads.
    source.set("now/a", Some("a2")).await;
    wait_for_now(&cache, "now/a", "a2").await;
}

/// A tokio mutex lock polled once fails when the task's coop budget is spent, though the mutex
/// is free; a read that does not wait still answers.
#[tokio::test]
async fn test_read_now_with_spent_coop_budget() {
    let source = TestSource::new();
    source.set("budget/a", Some("a")).await;
    let cache = Cache::<TestConfig>::new(source.clone(), "budget", "read-now-budget").await;
    wait_for_now(&cache, "budget/a", "a").await;

    let free = tokio::sync::Mutex::new(());
    let spent = (0..10_000).any(|_| free.lock().now_or_never().is_none());
    assert!(
        spent,
        "a free tokio mutex always locked: the coop budget was never spent"
    );

    let a = cache
        .try_get_now("budget/a")
        .unwrap()
        .unwrap()
        .map(|v| v.data);
    assert_eq!(a.as_deref(), Some("a"));
}

/// A source whose stream never delivers an event: the cache stays initializing.
#[derive(Debug, Clone)]
struct SilentSource;

#[async_trait::async_trait]
impl Source<Val> for SilentSource {
    async fn subscribe(
        &self,
        _left: &str,
        _right: &str,
    ) -> Result<EventStream<Val>, SubscribeError> {
        Ok(stream::pending().boxed())
    }
}

#[derive(Debug, Default)]
struct SilentConfig;

impl TypeConfig for SilentConfig {
    type Value = Val;
    type Source = SilentSource;

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

/// While the cache initializes, a read that does not wait answers `None` at once; a read that
/// waits does not answer.
#[tokio::test]
async fn test_read_now_while_initializing() {
    let cache = Cache::<SilentConfig>::new(SilentSource, "silent", "read-now-silent").await;

    assert!(cache.try_get_now("silent/a").is_none());
    assert!(cache.try_list_dir_now("silent").is_none());
    assert!(cache.try_access_now(|data| data.last_seq).is_none());

    let waiting = tokio::time::timeout(Duration::from_millis(100), cache.try_get("silent/a"));
    assert!(
        waiting.await.is_err(),
        "a read that waits answered an initializing cache"
    );
}
