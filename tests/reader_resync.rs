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

//! `CacheReader`: shared non-blocking reads, and `request_resync` recovering a stalled stream.

use std::time::Duration;

use sub_cache::testing::source::TestSource;
use sub_cache::testing::source::Val;
use sub_cache::testing::types::TestConfig;
use sub_cache::Cache;
use sub_cache::CacheReader;
use tokio::time::sleep;

fn read(reader: &CacheReader<TestConfig>, key: &str) -> Option<Option<Val>> {
    reader
        .try_access_now(|d| d.data.get(key).cloned())
        .and_then(Result::ok)
}

async fn wait_for(reader: &CacheReader<TestConfig>, key: &str, want: Option<Val>) {
    for _ in 0..100 {
        if read(reader, key) == Some(want.clone()) {
            return;
        }
        sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "{key} did not become {want:?}, last {:?}",
        read(reader, key)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_readers_share_access() {
    let source = TestSource::new();
    source.set("/k1", Some("v1")).await;
    let cache = Cache::<TestConfig>::new(source.clone(), "", "reader-shared").await;
    let reader = cache.reader();
    wait_for(&reader, "/k1", Some(Val::new(1, "v1"))).await;

    // Hold one read access while others read: shared access must not make them fall back.
    let held = reader.clone();
    let others = reader.clone();
    let got = held
        .try_access_now(|_| read(&others, "/k1"))
        .expect("readable")
        .expect("initialized");
    assert_eq!(got, Some(Some(Val::new(1, "v1"))));

    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let r = reader.clone();
            tokio::spawn(async move {
                let mut hits = 0;
                for _ in 0..1000 {
                    if read(&r, "/k1").is_some() {
                        hits += 1;
                    }
                }
                hits
            })
        })
        .collect();
    for t in tasks {
        assert_eq!(t.await.unwrap(), 1000);
    }
    drop(cache);
}

#[tokio::test]
async fn test_request_resync_recovers_stalled_stream() {
    let source = TestSource::new();
    source.set("/k1", Some("v1")).await;
    let cache = Cache::<TestConfig>::new(source.clone(), "", "reader-resync").await;
    let reader = cache.reader();
    wait_for(&reader, "/k1", Some(Val::new(1, "v1"))).await;

    // The source moves on but the stream delivers nothing: the cache stays stale.
    source.set_silently("/k1", Some("v2")).await;
    source.set_silently("/k2", Some("x")).await;
    sleep(Duration::from_millis(200)).await;
    assert_eq!(read(&reader, "/k1"), Some(Some(Val::new(1, "v1"))));
    assert_eq!(read(&reader, "/k2"), Some(None));

    // Resync drops the stream and re-fetches the full range.
    reader.request_resync();
    wait_for(&reader, "/k1", Some(Val::new(2, "v2"))).await;
    wait_for(&reader, "/k2", Some(Val::new(3, "x"))).await;

    // Events flow again on the new stream.
    source.set("/k1", None).await;
    wait_for(&reader, "/k1", None).await;
    drop(cache);
}
