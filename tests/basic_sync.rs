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

//! Basic synchronization integration test
//! Start watcher, change data source, verify local cache automatically syncs

use std::time::Duration;

use sub_cache::testing::source::TestSource;
use sub_cache::testing::source::Val;
use sub_cache::testing::types::TestConfig;
use sub_cache::testing::util::check_cache_state;
use sub_cache::Cache;
use tokio::time::sleep;

#[tokio::test]
async fn test_basic_sync() {
    // 1. Start a mock data source and a Cache
    let source = TestSource::new();
    let mut cache = Cache::<TestConfig>::new(source.clone(), "", "integration-basic-sync").await;

    // 2. Insert/modify/delete data in the data source
    source.set("/k1", Some("v1")).await;
    source.set("/k2", Some("v2")).await;
    source.set("/k1", Some("v1x")).await;
    source.set("/k2", None).await;

    sleep(Duration::from_millis(50)).await;

    // 3. Wait for cache sync (polling wait, max 1 second)
    let mut ok = false;
    for _ in 0..20 {
        let v1 = cache.try_get("/k1").await.unwrap();
        let v2 = cache.try_get("/k2").await.unwrap();
        if v1 == Some(Val::new(3, "v1x")) && v2.is_none() {
            ok = true;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert!(ok, "cache did not sync in time");

    // 4. Check that local cache content matches data source using utility function
    let cache_check_result = check_cache_state(&mut cache, 3, &[
        ("/k1", Some(Val::new(3, "v1x"))),
        ("/k2", None),
    ])
    .await
    .unwrap();

    match cache_check_result {
        Ok(()) => {} // All conditions satisfied
        Err(error_msg) => panic!("Cache state check failed: {error_msg}"),
    }
}
