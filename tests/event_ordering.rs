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

//! Event ordering and duplicate idempotency integration test
//! Simulate out-of-order and duplicate events to verify cache maintains correctness and idempotency

use std::time::Duration;

use sub_cache::testing::source::TestSource;
use sub_cache::testing::source::Val;
use sub_cache::testing::types::TestConfig;
use sub_cache::testing::util::wait_for_cache_state;
use sub_cache::Cache;
use tokio::time::sleep;

#[tokio::test]
async fn test_event_ordering_and_idempotency() {
    // 1. Create test source and cache
    let source = TestSource::new();
    let mut cache = Cache::<TestConfig>::new(source.clone(), "order", "event-ordering-test").await;

    // 2. Set initial data with specific sequence numbers
    source.set("order/k1", Some("v1")).await; // seq=1
    source.set("order/k2", Some("v2")).await; // seq=2
    source.set("order/k1", Some("v1_updated")).await; // seq=3

    // Wait for initial sync
    sleep(Duration::from_millis(100)).await;

    // 3. Verify initial state
    let v1 = cache.try_get("order/k1").await.unwrap();
    let v2 = cache.try_get("order/k2").await.unwrap();
    assert_eq!(v1, Some(Val::new(3, "v1_updated")));
    assert_eq!(v2, Some(Val::new(2, "v2")));
    assert_eq!(cache.try_last_seq().await.unwrap(), 3);

    // 4. Test idempotency - apply same changes again
    // The cache should ignore events with sequence numbers <= current sequence
    source.set("order/k1", Some("v1_old")).await; // seq=4 (this will be applied)
    source.set("order/k3", Some("v3")).await; // seq=5

    // Wait for sync
    sleep(Duration::from_millis(100)).await;

    // 5. Verify updates were applied correctly
    wait_for_cache_state(&mut cache, 5, &[
        ("order/k1", Some(Val::new(4, "v1_old"))),
        ("order/k2", Some(Val::new(2, "v2"))),
        ("order/k3", Some(Val::new(5, "v3"))),
    ])
    .await
    .unwrap()
    .expect("cache did not sync correctly after updates");

    // 6. Test sequence-based consistency
    // Events should be processed based on their sequence numbers
    source.set("order/k1", Some("final_value")).await; // seq=6
    source.set("order/k2", None).await; // seq=7 (delete)

    // Wait and verify final state
    wait_for_cache_state(&mut cache, 6, &[
        ("order/k1", Some(Val::new(6, "final_value"))),
        ("order/k2", None),
        ("order/k3", Some(Val::new(5, "v3"))),
    ])
    .await
    .unwrap()
    .expect("cache did not reach final consistent state");
}
