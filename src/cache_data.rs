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

use std::collections::BTreeMap;

use log::debug;
use log::warn;

use crate::TypeConfig;

/// The local data that reflects a range of key-values on remote data store.
#[derive(Debug, Clone)]
pub struct CacheData<C: TypeConfig> {
    /// The last sequence number ever seen from the remote data store.
    pub last_seq: u64,

    /// The key-value data stored in the cache.
    pub data: BTreeMap<String, C::Value>,
}

impl<C> Default for CacheData<C>
where C: TypeConfig
{
    fn default() -> Self {
        CacheData {
            last_seq: 0,
            data: BTreeMap::new(),
        }
    }
}

impl<C> CacheData<C>
where C: TypeConfig
{
    /// Process the watch response and update the local cache.
    ///
    /// Returns the new last_seq.
    pub(crate) fn apply_update(
        &mut self,
        key: String,
        prev: Option<C::Value>,
        current: Option<C::Value>,
    ) -> u64 {
        debug!(
            "Cache process update(key: {}, prev: {:?}, current: {:?})",
            key, prev, current
        );

        match (prev, current) {
            (_, Some(entry)) => {
                let entry_seq = C::value_seq(&entry);
                if entry_seq > self.last_seq {
                    self.last_seq = entry_seq;
                }

                self.data.insert(key, entry);
            }

            (Some(_entry), None) => {
                self.data.remove(&key);
            }

            (None, None) => {
                warn!(
                    "both prev and current are None when Cache processing watch response; Not possible, but ignoring"
                );
            }
        }

        self.last_seq
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use super::*;
    use crate::errors::SubscribeError;
    use crate::event_stream::EventStream;
    use crate::testing::source::Val;

    // Mock TypeConfig for testing
    #[derive(Debug, Default)]
    struct TestConfig;

    // Mock Source implementation
    #[derive(Debug)]
    struct MockSource;

    #[async_trait::async_trait]
    impl crate::type_config::Source<Val> for MockSource {
        async fn subscribe(
            &self,
            _left: &str,
            _right: &str,
        ) -> Result<EventStream<Val>, SubscribeError> {
            unimplemented!("Mock implementation for testing")
        }
    }

    impl TypeConfig for TestConfig {
        type Value = Val;
        type Source = MockSource;

        fn value_seq(value: &Self::Value) -> u64 {
            value.seq
        }

        fn spawn<F>(_future: F, _name: impl ToString)
        where
            F: Future + Send + 'static,
            F::Output: Send + 'static,
        {
            // Mock implementation - do nothing for tests
        }
    }

    #[test]
    fn test_default() {
        let cache_data: CacheData<TestConfig> = CacheData::default();

        assert_eq!(cache_data.last_seq, 0);
        assert!(cache_data.data.is_empty());
    }

    #[test]
    fn test_apply_update_scenarios() {
        let mut cache_data: CacheData<TestConfig> = CacheData::default();

        // Insert: None -> Some
        let new_value = Val::new(5, "test");
        let seq = cache_data.apply_update("key1".to_string(), None, Some(new_value.clone()));
        assert_eq!(seq, 5);
        assert_eq!(cache_data.last_seq, 5);
        assert_eq!(cache_data.data.get("key1"), Some(&new_value));

        // Update: Some -> Some
        let updated_value = Val::new(10, "updated");
        let seq = cache_data.apply_update(
            "key1".to_string(),
            Some(Val::new(5, "test")),
            Some(updated_value.clone()),
        );
        assert_eq!(seq, 10);
        assert_eq!(cache_data.last_seq, 10);
        assert_eq!(cache_data.data.get("key1"), Some(&updated_value));

        // Delete: Some -> None
        cache_data.apply_update("key1".to_string(), Some(Val::new(10, "updated")), None);
        assert!(!cache_data.data.contains_key("key1"));

        // Invalid: None -> None (should not crash, just warn)
        let seq_before = cache_data.last_seq;
        cache_data.apply_update("key2".to_string(), None, None);
        assert_eq!(cache_data.last_seq, seq_before); // Should remain unchanged
        assert!(!cache_data.data.contains_key("key2"));
    }

    #[test]
    fn test_last_seq_not_decreased() {
        let mut cache_data: CacheData<TestConfig> = CacheData::default();

        // Insert with seq 10
        let v10 = Val::new(10, "v10");
        cache_data.apply_update("k".to_string(), None, Some(v10.clone()));
        assert_eq!(cache_data.last_seq, 10);

        // Update with seq 5 (should not decrease last_seq)
        let v5 = Val::new(5, "v5");
        cache_data.apply_update("k".to_string(), Some(v10.clone()), Some(v5.clone()));
        assert_eq!(cache_data.last_seq, 10);

        // Update with seq 20 (should increase last_seq)
        let v20 = Val::new(20, "v20");
        cache_data.apply_update("k".to_string(), Some(v5.clone()), Some(v20.clone()));
        assert_eq!(cache_data.last_seq, 20);
    }
}
