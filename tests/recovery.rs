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

//! Recovery integration test
//!
//! This test validates that a distributed cache can properly recover after connection failures.
//! The key aspects being tested are:
//!
//! 1. **Connection Drop Detection**: Cache detects when connection is lost
//! 2. **Automatic Reconnection**: Cache automatically reconnects after connection failure
//! 3. **State Recovery**: Cache recovers to the latest state after reconnection
//! 4. **Consistency After Recovery**: Cache maintains consistency after recovery
//! 5. **Continued Operation**: Cache can continue receiving updates after recovery
//!
//! The test simulates a production scenario where network connections fail and need
//! to be re-established while maintaining data consistency.

use std::time::Duration;

use sub_cache::testing::source::TestSource;
use sub_cache::testing::source::Val;
use sub_cache::testing::types::TestConfig;
use sub_cache::testing::util::check_cache_state;
use sub_cache::testing::util::retry_check_cache_state;
use sub_cache::testing::util::wait_for_cache_state;
use sub_cache::Cache;
use tokio::time::sleep;

#[tokio::test]
async fn test_recovery_after_crash() {
    // 1. Create test source and populate with initial data
    let source = TestSource::new();

    // Pre-populate the source with initial data
    source.set("recovery/key1", Some("initial_value1")).await;
    source.set("recovery/key2", Some("initial_value2")).await;
    source.set("recovery/key3", Some("initial_value3")).await;

    // 2. Create cache and verify it syncs initial data
    let mut cache = Cache::<TestConfig>::new(source.clone(), "recovery", "recovery-cache").await;

    // Wait for initial synchronization
    sleep(Duration::from_millis(200)).await;

    // Verify initial data is synced using convenience function
    wait_for_cache_state(&mut cache, 3, &[
        ("recovery/key1", Some(Val::new(1, "initial_value1"))),
        ("recovery/key2", Some(Val::new(2, "initial_value2"))),
        ("recovery/key3", Some(Val::new(3, "initial_value3"))),
    ])
    .await
    .unwrap()
    .expect("initial cache did not sync properly");

    // 3. Write more data while cache is running normally
    source.set("recovery/key1", Some("updated_value1")).await;
    source.set("recovery/key4", Some("new_value4")).await;

    // Wait for updates to sync
    sleep(Duration::from_millis(100)).await;

    // Verify updates are synced using convenience function
    wait_for_cache_state(&mut cache, 5, &[
        ("recovery/key1", Some(Val::new(4, "updated_value1"))),
        ("recovery/key4", Some(Val::new(5, "new_value4"))),
    ])
    .await
    .unwrap()
    .expect("cache did not sync updates properly");

    // 4. Simulate connection failure by dropping all connections
    // This will cause the cache to receive ConnectionClosed errors
    source
        .drop_all_connections("simulated network failure")
        .await;

    // 5. Write data while connection is down (cache should miss these initially)
    source
        .set("recovery/key2", Some("post_failure_value2"))
        .await;
    source
        .set("recovery/key5", Some("post_failure_value5"))
        .await;
    source.set("recovery/key3", None).await; // Delete key3

    // 6. Wait for cache to detect connection failure and reconnect
    // The cache should automatically reconnect and recover to the latest state
    sleep(Duration::from_millis(500)).await;

    // 7. Verify cache recovers to the latest state (including changes made during connection failure)
    // Give more time for recovery (50 attempts * 100ms = 5 seconds)
    retry_check_cache_state(
        &mut cache,
        7,
        &[
            ("recovery/key1", Some(Val::new(4, "updated_value1"))),
            ("recovery/key2", Some(Val::new(6, "post_failure_value2"))),
            ("recovery/key3", None), // key3 was deleted
            ("recovery/key4", Some(Val::new(5, "new_value4"))),
            ("recovery/key5", Some(Val::new(7, "post_failure_value5"))),
        ],
        50,
        Duration::from_millis(100),
    )
    .await
    .unwrap()
    .expect("cache did not recover to latest state after connection failure");

    // 8. Verify cache can continue to receive new updates after recovery
    source.set("recovery/key1", Some("final_value1")).await;
    source.set("recovery/key6", Some("final_value6")).await;

    // Wait for new updates to sync
    sleep(Duration::from_millis(200)).await;

    // Verify new updates are processed correctly using convenience function
    wait_for_cache_state(&mut cache, 10, &[
        ("recovery/key1", Some(Val::new(9, "final_value1"))),
        ("recovery/key6", Some(Val::new(10, "final_value6"))),
    ])
    .await
    .unwrap()
    .expect("cache did not sync new updates after recovery");

    // 9. Test multiple connection failures to ensure robust recovery
    // Simulate another connection failure
    source.drop_all_connections("second network failure").await;

    // Write more data during second failure
    source
        .set("recovery/key7", Some("second_failure_value"))
        .await;
    source
        .set("recovery/key1", Some("twice_updated_value1"))
        .await;

    // Wait for second recovery
    sleep(Duration::from_millis(500)).await;

    // Verify second recovery using retry utility function
    retry_check_cache_state(
        &mut cache,
        12,
        &[
            ("recovery/key1", Some(Val::new(12, "twice_updated_value1"))),
            ("recovery/key7", Some(Val::new(11, "second_failure_value"))),
        ],
        30,
        Duration::from_millis(100),
    )
    .await
    .unwrap()
    .expect("cache did not recover from second connection failure");

    // 10. Final verification of complete state using utility function
    check_cache_state(&mut cache, 12, &[
        ("recovery/key1", Some(Val::new(12, "twice_updated_value1"))),
        ("recovery/key2", Some(Val::new(6, "post_failure_value2"))),
        ("recovery/key3", None), // Deleted during first failure
        ("recovery/key4", Some(Val::new(5, "new_value4"))),
        ("recovery/key5", Some(Val::new(7, "post_failure_value5"))),
        ("recovery/key6", Some(Val::new(10, "final_value6"))),
        ("recovery/key7", Some(Val::new(11, "second_failure_value"))),
    ])
    .await
    .unwrap()
    .unwrap();
}
