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

//! Multi-watcher concurrent collaboration integration test
//!
//! This test validates that multiple cache instances (watchers) can subscribe to
//! overlapping key ranges and maintain consistency when data is written concurrently.
//! The key aspects being tested are:
//!
//! 1. **Overlapping Subscriptions**: Multiple caches subscribe to overlapping key ranges
//! 2. **Concurrent Writes**: Data is written concurrently to a shared data source
//! 3. **Cross-Cache Consistency**: All caches see the same final state
//! 4. **Real-time Synchronization**: Changes propagate to all relevant caches
//! 5. **Range Isolation**: Caches only see data within their subscribed ranges
//! 6. **Sequence Consistency**: All caches maintain proper sequence ordering
//!
//! The test simulates a distributed system where multiple cache instances
//! monitor overlapping data ranges and must stay synchronized.

use std::time::Duration;

use sub_cache::testing::source::TestSource;
use sub_cache::testing::source::Val;
use sub_cache::testing::types::TestConfig;
use sub_cache::testing::util::wait_for_cache_state;
use sub_cache::Cache;
use tokio::time::sleep;

#[tokio::test]
async fn test_multi_watcher_concurrent() {
    // 1. Create a single shared test source (this is the correct approach for testing concurrency)
    let source = TestSource::new();

    // 2. Pre-populate some initial data to avoid initialization conflicts
    source.set("app/user/initial", Some("initial_user")).await;
    source
        .set("app/config/initial", Some("initial_config"))
        .await;
    source.set("system/initial", Some("initial_system")).await;

    // 3. Create multiple cache instances with overlapping ranges
    // Cache A: monitors "app/user/" range
    let mut cache_a = Cache::<TestConfig>::new(source.clone(), "app/user", "cache-a-users").await;

    // Cache B: monitors "app/" range (includes users and other data)
    let mut cache_b = Cache::<TestConfig>::new(source.clone(), "app", "cache-b-app").await;

    // Wait for both caches to initialize and see the initial data
    wait_for_cache_state(&mut cache_a, 1, &[(
        "app/user/initial",
        Some(Val::new(1, "initial_user")),
    )])
    .await
    .unwrap()
    .unwrap();
    wait_for_cache_state(&mut cache_b, 2, &[
        ("app/user/initial", Some(Val::new(1, "initial_user"))),
        ("app/config/initial", Some(Val::new(2, "initial_config"))),
    ])
    .await
    .unwrap()
    .unwrap();

    // 4. Test concurrent writes to the shared source
    let write_task_1 = tokio::spawn({
        let source = source.clone();
        async move {
            // Write user data (should appear in both cache_a and cache_b)
            for i in 1..=3 {
                let key = format!("app/user/user{i}");
                let value = format!("user_data_{i}");
                source.set(&key, Some(&value)).await;
                sleep(Duration::from_millis(20)).await;
            }
        }
    });

    let write_task_2 = tokio::spawn({
        let source = source.clone();
        async move {
            // Write config data (should appear in cache_b only)
            for i in 1..=3 {
                let key = format!("app/config/setting{i}");
                let value = format!("config_value_{i}");
                source.set(&key, Some(&value)).await;
                sleep(Duration::from_millis(25)).await;
            }
        }
    });

    let write_task_3 = tokio::spawn({
        let source = source.clone();
        async move {
            // Write system data (should not appear in either cache)
            for i in 1..=2 {
                let key = format!("system/service{i}");
                let value = format!("service_status_{i}");
                source.set(&key, Some(&value)).await;
                sleep(Duration::from_millis(30)).await;
            }
        }
    });

    // 5. Wait for all concurrent writes to complete
    let _ = tokio::join!(write_task_1, write_task_2, write_task_3);

    // 6. Wait for synchronization across all caches
    sleep(Duration::from_millis(300)).await;

    // 7. Verify cache_a (app/user/ range) sees user data but not config/system data
    let cache_a_entries = cache_a.try_list_dir("app/user").await.unwrap();
    assert_eq!(
        cache_a_entries.len(),
        4,
        "Cache A should have 4 user entries (1 initial + 3 new)"
    );

    // Check that cache_a sees all user data
    assert!(cache_a.try_get("app/user/initial").await.unwrap().is_some());
    for i in 1..=3 {
        let key = format!("app/user/user{i}");
        let value = cache_a.try_get(&key).await.unwrap().unwrap();
        assert_eq!(value.data, format!("user_data_{i}"));
    }

    // Cache A should not see config or system data
    assert_eq!(cache_a.try_get("app/config/initial").await.unwrap(), None);
    assert_eq!(cache_a.try_get("app/config/setting1").await.unwrap(), None);
    assert_eq!(cache_a.try_get("system/service1").await.unwrap(), None);

    // 8. Verify cache_b (app/ range) sees both user and config data but not system data
    let cache_b_entries = cache_b.try_list_dir("app").await.unwrap();
    assert_eq!(
        cache_b_entries.len(),
        8,
        "Cache B should have 8 app entries (2 initial + 3 users + 3 configs)"
    );

    // Check that cache_b sees all user and config data
    assert!(cache_b.try_get("app/user/initial").await.unwrap().is_some());
    assert!(cache_b
        .try_get("app/config/initial")
        .await
        .unwrap()
        .is_some());

    for i in 1..=3 {
        let user_key = format!("app/user/user{i}");
        let user_value = cache_b.try_get(&user_key).await.unwrap().unwrap();
        assert_eq!(user_value.data, format!("user_data_{i}"));

        let config_key = format!("app/config/setting{i}");
        let config_value = cache_b.try_get(&config_key).await.unwrap().unwrap();
        assert_eq!(config_value.data, format!("config_value_{i}"));
    }

    // Cache B should not see system data
    assert_eq!(cache_b.try_get("system/service1").await.unwrap(), None);

    // 9. Verify sequence consistency between overlapping caches
    let seq_a = cache_a.try_last_seq().await.unwrap();
    let seq_b = cache_b.try_last_seq().await.unwrap();

    assert!(seq_a > 0, "Cache A should have positive sequence");
    assert!(seq_b > 0, "Cache B should have positive sequence");
    assert!(
        seq_b >= seq_a,
        "Cache B should have sequence >= Cache A (sees more data)"
    );

    // 10. Test concurrent updates to overlapping data
    let update_task_1 = tokio::spawn({
        let source = source.clone();
        async move {
            // Update user data (should appear in both caches)
            source.set("app/user/user1", Some("updated_user_1")).await;
            source
                .set("app/user/initial", Some("updated_initial_user"))
                .await;
        }
    });

    let update_task_2 = tokio::spawn({
        let source = source.clone();
        async move {
            // Update config data (should appear in cache_b only)
            source
                .set("app/config/setting1", Some("updated_config_1"))
                .await;
            source
                .set("app/config/initial", Some("updated_initial_config"))
                .await;
        }
    });

    let update_task_3 = tokio::spawn({
        let source = source.clone();
        async move {
            // Delete and recreate some data
            source.set("app/user/user2", None).await;
            sleep(Duration::from_millis(50)).await;
            source.set("app/user/user2", Some("recreated_user_2")).await;
        }
    });

    // Wait for all updates
    let _ = tokio::join!(update_task_1, update_task_2, update_task_3);

    // Wait for synchronization
    sleep(Duration::from_millis(300)).await;

    // 11. Verify updates are reflected correctly in both caches
    // Check user updates in both caches (both should see the same data from the shared source)
    let user1_a = cache_a.try_get("app/user/user1").await.unwrap().unwrap();
    let user1_b = cache_b.try_get("app/user/user1").await.unwrap().unwrap();
    assert_eq!(user1_a.data, "updated_user_1");
    assert_eq!(user1_b.data, "updated_user_1");
    assert_eq!(
        user1_a.seq, user1_b.seq,
        "Same data from shared source should have same sequence"
    );

    let initial_user_a = cache_a.try_get("app/user/initial").await.unwrap().unwrap();
    let initial_user_b = cache_b.try_get("app/user/initial").await.unwrap().unwrap();
    assert_eq!(initial_user_a.data, "updated_initial_user");
    assert_eq!(initial_user_b.data, "updated_initial_user");
    assert_eq!(
        initial_user_a.seq, initial_user_b.seq,
        "Same data should have same sequence"
    );

    // Check config updates (only cache_b should see these)
    assert_eq!(cache_a.try_get("app/config/setting1").await.unwrap(), None);
    let config1_b = cache_b
        .try_get("app/config/setting1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(config1_b.data, "updated_config_1");

    let initial_config_b = cache_b
        .try_get("app/config/initial")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial_config_b.data, "updated_initial_config");

    // Check recreation
    let user2_a = cache_a.try_get("app/user/user2").await.unwrap().unwrap();
    let user2_b = cache_b.try_get("app/user/user2").await.unwrap().unwrap();
    assert_eq!(user2_a.data, "recreated_user_2");
    assert_eq!(user2_b.data, "recreated_user_2");
    assert_eq!(
        user2_a.seq, user2_b.seq,
        "Same recreated data should have same sequence"
    );

    // 12. Final consistency verification
    let (a_count, a_max_seq) = cache_a
        .try_access(|data| (data.data.len(), data.last_seq))
        .await
        .unwrap();
    let (b_count, b_max_seq) = cache_b
        .try_access(|data| (data.data.len(), data.last_seq))
        .await
        .unwrap();

    // Verify final counts (cache_a: 4 user entries, cache_b: 8 app entries)
    assert_eq!(a_count, 4, "Cache A should have 4 user entries");
    assert_eq!(b_count, 8, "Cache B should have 8 app entries");

    // 13. Test rapid concurrent operations
    let rapid_writes = tokio::spawn({
        let source = source.clone();
        async move {
            for i in 50..55 {
                source
                    .set(
                        &format!("app/user/rapid{i}"),
                        Some(&format!("rapid_value_{i}")),
                    )
                    .await;
                sleep(Duration::from_millis(5)).await;
            }
        }
    });

    let rapid_reads_a = tokio::spawn({
        let mut cache_a = cache_a;
        async move {
            for _ in 0..10 {
                let _ = cache_a.try_get("app/user/user1").await;
                let _ = cache_a.try_list_dir("app/user").await;
                sleep(Duration::from_millis(8)).await;
            }
            cache_a
        }
    });

    let rapid_reads_b = tokio::spawn({
        let mut cache_b = cache_b;
        async move {
            for _ in 0..10 {
                let _ = cache_b.try_get("app/config/setting1").await;
                let _ = cache_b.try_list_dir("app").await;
                sleep(Duration::from_millis(6)).await;
            }
            cache_b
        }
    });

    let (_, cache_a, cache_b) = tokio::join!(rapid_writes, rapid_reads_a, rapid_reads_b);
    let mut cache_a = cache_a.unwrap();
    let mut cache_b = cache_b.unwrap();

    // Wait for final synchronization
    sleep(Duration::from_millis(200)).await;

    // Verify rapid writes were seen
    let final_entries_a = cache_a.try_list_dir("app/user").await.unwrap();
    let final_entries_b = cache_b.try_list_dir("app/user").await.unwrap();

    assert!(
        final_entries_a.len() >= 9,
        "Cache A should see original + rapid user entries"
    );
    assert!(
        final_entries_b.len() >= 9,
        "Cache B should see original + rapid user entries"
    );

    println!("Multi-watcher concurrent test completed successfully!");
    println!("Cache A: {a_count} entries, seq {a_max_seq}");
    println!("Cache B: {b_count} entries, seq {b_max_seq}");
    println!("Final Cache A entries: {}", final_entries_a.len());
    println!("Final Cache B user entries: {}", final_entries_b.len());
    println!("All caches maintained consistency during concurrent operations with shared source!");
}
