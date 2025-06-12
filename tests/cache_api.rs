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

//! Cache API integration test
//!
//! This test validates the basic public API methods of the Cache struct to ensure
//! they work correctly in isolation and in combination. The key aspects being tested are:
//!
//! 1. **Cache Creation**: Proper initialization and background task spawning
//! 2. **Data Retrieval**: `try_get()` method for individual key access
//! 3. **Sequence Access**: `try_last_seq()` method for sequence number retrieval
//! 4. **Directory Listing**: `try_list_dir()` method for range queries
//! 5. **Atomic Access**: `try_access()` method for consistent multi-operation access
//! 6. **Error Handling**: Proper error propagation and unsupported operation handling
//! 7. **State Consistency**: Cache reflects data source changes correctly
//!
//! The test covers both successful operations and edge cases to ensure robust API behavior.

use std::time::Duration;

use sub_cache::testing::source::TestSource;
use sub_cache::testing::types::TestConfig;
use sub_cache::testing::util::wait_for_cache_state;
use sub_cache::Cache;
use tokio::time::sleep;

#[tokio::test]
async fn test_cache_api_methods() {
    // 1. Test cache creation and initialization
    let source = TestSource::new();
    let mut cache = Cache::<TestConfig>::new(source.clone(), "api_test", "cache-api-test").await;

    // Cache should be initialized and ready to use
    // Initial state should have sequence 0 and no data
    wait_for_cache_state(&mut cache, 0, &[])
        .await
        .unwrap()
        .unwrap();

    // 2. Test try_get() method with empty cache
    let result = cache.try_get("api_test/nonexistent").await.unwrap();
    assert_eq!(result, None, "Non-existent key should return None");

    // 3. Test try_last_seq() method with empty cache
    let seq = cache.try_last_seq().await.unwrap();
    assert_eq!(seq, 0, "Initial sequence should be 0");

    // 4. Test try_list_dir() method with empty cache
    let entries = cache.try_list_dir("api_test").await.unwrap();
    assert_eq!(entries.len(), 0, "Empty cache should return empty list");

    // 5. Add some test data to the source
    source.set("api_test/key1", Some("value1")).await;
    source.set("api_test/key2", Some("value2")).await;
    source.set("api_test/subdir/key3", Some("value3")).await;
    source.set("other_prefix/key4", Some("value4")).await; // Outside our prefix

    // Wait for cache synchronization
    sleep(Duration::from_millis(100)).await;

    // 6. Test try_get() method with existing data
    let val1 = cache.try_get("api_test/key1").await.unwrap().unwrap();
    assert_eq!(val1.data, "value1");
    assert!(val1.seq > 0, "Sequence should be positive");

    let val2 = cache.try_get("api_test/key2").await.unwrap().unwrap();
    assert_eq!(val2.data, "value2");
    assert!(val2.seq > val1.seq, "Sequence should be increasing");

    let val3 = cache
        .try_get("api_test/subdir/key3")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(val3.data, "value3");

    // Key outside our prefix should not be accessible
    let val4 = cache.try_get("other_prefix/key4").await.unwrap();
    assert_eq!(val4, None, "Key outside prefix should not be accessible");

    // 7. Test try_last_seq() method with data
    let current_seq = cache.try_last_seq().await.unwrap();
    assert!(current_seq >= 3, "Sequence should reflect all updates");
    assert_eq!(
        current_seq, val3.seq,
        "Last sequence should match latest value"
    );

    // 8. Test try_list_dir() method with data
    let all_entries = cache.try_list_dir("api_test").await.unwrap();
    assert_eq!(all_entries.len(), 3, "Should list all entries in prefix");

    // Verify entries are sorted by key
    let keys: Vec<String> = all_entries.iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(keys, vec![
        "api_test/key1",
        "api_test/key2",
        "api_test/subdir/key3"
    ]);

    // Verify values match
    assert_eq!(all_entries[0].1.data, "value1");
    assert_eq!(all_entries[1].1.data, "value2");
    assert_eq!(all_entries[2].1.data, "value3");

    // 9. Test try_list_dir() with subdirectory
    let subdir_entries = cache.try_list_dir("api_test/subdir").await.unwrap();
    assert_eq!(subdir_entries.len(), 1, "Should list only subdir entries");
    assert_eq!(subdir_entries[0].0, "api_test/subdir/key3");
    assert_eq!(subdir_entries[0].1.data, "value3");

    // 10. Test try_list_dir() with non-existent directory
    let empty_entries = cache.try_list_dir("api_test/nonexistent").await.unwrap();
    assert_eq!(
        empty_entries.len(),
        0,
        "Non-existent directory should return empty list"
    );

    // 11. Test try_access() method for atomic operations
    let (retrieved_val1, retrieved_val2, retrieved_seq) = cache
        .try_access(|cache_data| {
            let val1 = cache_data.data.get("api_test/key1").cloned();
            let val2 = cache_data.data.get("api_test/key2").cloned();
            let seq = cache_data.last_seq;
            (val1, val2, seq)
        })
        .await
        .unwrap();

    assert_eq!(retrieved_val1.unwrap().data, "value1");
    assert_eq!(retrieved_val2.unwrap().data, "value2");
    assert_eq!(retrieved_seq, current_seq);

    // 12. Test data updates and cache synchronization
    source.set("api_test/key1", Some("updated_value1")).await;
    source.set("api_test/key2", None).await; // Delete key2

    // Wait for synchronization
    sleep(Duration::from_millis(100)).await;

    // Verify updates
    let updated_val1 = cache.try_get("api_test/key1").await.unwrap().unwrap();
    assert_eq!(updated_val1.data, "updated_value1");
    assert!(
        updated_val1.seq > current_seq,
        "Updated value should have higher sequence"
    );

    let deleted_val2 = cache.try_get("api_test/key2").await.unwrap();
    assert_eq!(deleted_val2, None, "Deleted key should return None");

    // 13. Test sequence progression
    let new_seq = cache.try_last_seq().await.unwrap();
    assert!(
        new_seq > current_seq,
        "Sequence should progress after updates"
    );

    // 14. Test try_list_dir() after updates
    let updated_entries = cache.try_list_dir("api_test").await.unwrap();
    assert_eq!(updated_entries.len(), 2, "Should reflect deletions");

    let updated_keys: Vec<String> = updated_entries.iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(updated_keys, vec!["api_test/key1", "api_test/subdir/key3"]);
    assert_eq!(updated_entries[0].1.data, "updated_value1");

    // 15. Test edge cases with special characters and paths
    source
        .set("api_test/key with spaces", Some("space_value"))
        .await;
    source
        .set("api_test/key-with-dashes", Some("dash_value"))
        .await;
    source
        .set("api_test/key_with_underscores", Some("underscore_value"))
        .await;

    sleep(Duration::from_millis(100)).await;

    let space_val = cache
        .try_get("api_test/key with spaces")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(space_val.data, "space_value");

    let dash_val = cache
        .try_get("api_test/key-with-dashes")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dash_val.data, "dash_value");

    let underscore_val = cache
        .try_get("api_test/key_with_underscores")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(underscore_val.data, "underscore_value");

    // 16. Test final state consistency
    let final_entries = cache.try_list_dir("api_test").await.unwrap();
    assert_eq!(
        final_entries.len(),
        5,
        "Should include all special character keys"
    );

    let final_seq = cache.try_last_seq().await.unwrap();
    assert!(final_seq > new_seq, "Final sequence should be highest");

    // 17. Test atomic access with complex operations
    let (total_entries, max_seq, all_values) = cache
        .try_access(|cache_data| {
            let entries = cache_data.data.len();
            let max_seq = cache_data.last_seq;
            let values: Vec<String> = cache_data.data.values().map(|v| v.data.clone()).collect();
            (entries, max_seq, values)
        })
        .await
        .unwrap();

    assert!(total_entries >= 5, "Should have at least 5 entries total");
    assert_eq!(
        max_seq, final_seq,
        "Max sequence should match final sequence"
    );
    assert!(all_values.contains(&"updated_value1".to_string()));
    assert!(all_values.contains(&"value3".to_string()));
    assert!(all_values.contains(&"space_value".to_string()));

    println!("Cache API test completed successfully!");
    println!("Final state: {total_entries} entries, sequence {final_seq}");
}

#[tokio::test]
async fn test_cache_api_error_handling() {
    // Test cache behavior with various edge cases and error conditions
    let source = TestSource::new();
    let mut cache = Cache::<TestConfig>::new(source.clone(), "error_test", "error-test").await;

    // Wait for initialization
    wait_for_cache_state(&mut cache, 0, &[])
        .await
        .unwrap()
        .unwrap();

    // 1. Test accessing keys with invalid prefixes (should return None, not error)
    let invalid_key = cache.try_get("wrong_prefix/key").await.unwrap();
    assert_eq!(invalid_key, None, "Invalid prefix should return None");

    // 2. Test empty key access
    let empty_key = cache.try_get("").await.unwrap();
    assert_eq!(empty_key, None, "Empty key should return None");

    // 3. Test very long key names
    let long_key = format!("error_test/{}", "x".repeat(1000));
    source.set(&long_key, Some("long_value")).await;
    sleep(Duration::from_millis(50)).await;

    let long_val = cache.try_get(&long_key).await.unwrap().unwrap();
    assert_eq!(long_val.data, "long_value");

    // 4. Test special characters in values
    source
        .set(
            "error_test/special",
            Some("value with\nnewlines\tand\ttabs"),
        )
        .await;
    sleep(Duration::from_millis(50)).await;

    let special_val = cache.try_get("error_test/special").await.unwrap().unwrap();
    assert_eq!(special_val.data, "value with\nnewlines\tand\ttabs");

    // 5. Test rapid updates to same key
    for i in 0..10 {
        source
            .set("error_test/rapid", Some(&format!("rapid_value_{i}")))
            .await;
    }
    sleep(Duration::from_millis(100)).await;

    let rapid_val = cache.try_get("error_test/rapid").await.unwrap().unwrap();
    assert_eq!(rapid_val.data, "rapid_value_9");

    // 6. Test directory listing with various edge cases
    let empty_dir = cache
        .try_list_dir("error_test/nonexistent/deep/path")
        .await
        .unwrap();
    assert_eq!(empty_dir.len(), 0);

    let root_dir = cache.try_list_dir("").await.unwrap();
    // Should include entries from our prefix, but may be empty if no data exists
    // The important thing is that it doesn't error
    println!("Root directory entries: {}", root_dir.len());

    println!("Cache API error handling test completed successfully!");
}
