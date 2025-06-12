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
where
    C: TypeConfig,
{
    fn default() -> Self {
        CacheData {
            last_seq: 0,
            data: BTreeMap::new(),
        }
    }
}

impl<C> CacheData<C>
where
    C: TypeConfig,
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
                self.last_seq = C::value_seq(&entry);
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
        };

        self.last_seq
    }
}
