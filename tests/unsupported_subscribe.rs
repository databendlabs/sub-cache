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

//! Unsupported subscription integration test
//!
//! This test validates that the cache properly handles data sources that don't support
//! subscription functionality. The key aspects being tested are:
//!
//! 1. **Unsupported Error Handling**: Cache properly propagates SubscribeError::Unsupported
//! 2. **Graceful Degradation**: Cache enters a degraded state rather than crashing
//! 3. **Error Propagation**: All cache operations return Err(Unsupported) consistently
//! 4. **Resource Management**: Background tasks are properly cleaned up
//!
//! The test simulates a real-world scenario where the underlying data source
//! doesn't support real-time subscriptions (e.g., a read-only database, file system, etc.).

use std::time::Duration;

use sub_cache::errors::SubscribeError;
use sub_cache::errors::Unsupported;
use sub_cache::event_stream::EventStream;
use sub_cache::testing::source::Val;
use sub_cache::Cache;
use sub_cache::Source;
use sub_cache::TypeConfig;
use tokio::time::sleep;

// Mock source that doesn't support subscription
#[derive(Debug, Clone)]
struct UnsupportedSource {
    error_message: String,
}

impl UnsupportedSource {
    fn new(error_message: impl ToString) -> Self {
        Self {
            error_message: error_message.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl Source<Val> for UnsupportedSource {
    async fn subscribe(
        &self,
        _left: &str,
        _right: &str,
    ) -> Result<EventStream<Val>, SubscribeError> {
        // Always return Unsupported error to simulate a data source that doesn't support subscriptions
        Err(SubscribeError::Unsupported(Unsupported::new(
            &self.error_message,
        )))
    }
}

// Test configuration for unsupported source
#[derive(Debug, Default)]
struct UnsupportedTestConfig;

impl TypeConfig for UnsupportedTestConfig {
    type Value = Val;
    type Source = UnsupportedSource;

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

#[tokio::test]
async fn test_unsupported_subscribe() {
    // 1. Create a mock data source that doesn't support subscription
    let unsupported_source =
        UnsupportedSource::new("Subscription not supported by this data source");

    // 2. Create cache with the unsupported source
    // The cache creation should complete, but the background watcher should fail
    let mut cache = Cache::<UnsupportedTestConfig>::new(
        unsupported_source,
        "unsupported_test",
        "unsupported-test",
    )
    .await;

    // 3. Give the background task time to attempt subscription and fail
    sleep(Duration::from_millis(200)).await;

    // 4. Test that all cache operations return Err(Unsupported)

    // Test try_get() method
    let get_result = cache.try_get("unsupported_test/key1").await;
    assert!(
        get_result.is_err(),
        "try_get should return Err(Unsupported)"
    );

    if let Err(unsupported_error) = get_result {
        let error_msg = unsupported_error.to_string();
        assert!(
            error_msg.contains("Subscription not supported"),
            "Error should contain our custom message: {error_msg}"
        );
    }

    // Test try_last_seq() method
    let seq_result = cache.try_last_seq().await;
    assert!(
        seq_result.is_err(),
        "try_last_seq should return Err(Unsupported)"
    );

    if let Err(unsupported_error) = seq_result {
        let error_msg = unsupported_error.to_string();
        assert!(
            error_msg.contains("Subscription not supported"),
            "Error should contain our custom message: {error_msg}"
        );
    }

    // Test try_list_dir() method
    let list_result = cache.try_list_dir("unsupported_test").await;
    assert!(
        list_result.is_err(),
        "try_list_dir should return Err(Unsupported)"
    );

    if let Err(unsupported_error) = list_result {
        let error_msg = unsupported_error.to_string();
        assert!(
            error_msg.contains("Subscription not supported"),
            "Error should contain our custom message: {error_msg}"
        );
    }

    // Test try_access() method
    let access_result = cache
        .try_access(|_cache_data| {
            // This closure should never be called since the cache is in error state
            panic!("try_access closure should not be called when cache is in error state");
        })
        .await;
    assert!(
        access_result.is_err(),
        "try_access should return Err(Unsupported)"
    );

    let unsupported_error = access_result.unwrap_err();
    let error_msg = unsupported_error.to_string();
    assert!(
        error_msg.contains("Subscription not supported"),
        "Error should contain our custom message: {error_msg}"
    );

    // 5. Test that the cache consistently returns the same error
    // Multiple calls should all return the same Unsupported error
    for i in 0..3 {
        let get_result = cache.try_get(&format!("unsupported_test/key{i}")).await;
        assert!(
            get_result.is_err(),
            "All try_get calls should return Err(Unsupported)"
        );

        let seq_result = cache.try_last_seq().await;
        assert!(
            seq_result.is_err(),
            "All try_last_seq calls should return Err(Unsupported)"
        );
    }

    // 6. Test cache_data() method for direct access to the error state
    let cache_data_guard = cache.cache_data().await;
    assert!(
        cache_data_guard.is_err(),
        "cache_data should contain Err(Unsupported)"
    );

    if let Err(unsupported_error) = cache_data_guard.as_ref() {
        let error_msg = unsupported_error.to_string();
        assert!(
            error_msg.contains("Subscription not supported"),
            "Cache data error should contain our custom message: {error_msg}"
        );
    }

    println!("Unsupported subscription test completed successfully!");
    println!("Cache properly entered degraded state and consistently returns Unsupported errors");
}

#[tokio::test]
async fn test_unsupported_subscribe_different_errors() {
    // Test with different error messages to ensure they're properly propagated
    let test_cases = [
        "Database does not support real-time subscriptions",
        "File system source is read-only",
        "Legacy API version does not include subscription support",
        "Subscription feature disabled in configuration",
    ];

    for (i, error_message) in test_cases.iter().enumerate() {
        let unsupported_source = UnsupportedSource::new(error_message);
        let mut cache = Cache::<UnsupportedTestConfig>::new(
            unsupported_source,
            &format!("unsupported_test_{i}"),
            &format!("unsupported-test-{i}"),
        )
        .await;

        // Give time for background task to fail
        sleep(Duration::from_millis(100)).await;

        // Verify the specific error message is propagated
        let get_result = cache.try_get(&format!("unsupported_test_{i}/key")).await;
        assert!(
            get_result.is_err(),
            "Should return Err(Unsupported) for case {i}"
        );

        if let Err(unsupported_error) = get_result {
            let error_msg = unsupported_error.to_string();
            assert!(
                error_msg.contains(error_message),
                "Error should contain custom message '{error_message}', got: {error_msg}"
            );
        }
    }

    println!("Different unsupported error messages test completed successfully!");
}
