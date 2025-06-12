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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change<V> {
    pub key: String,
    pub before: Option<V>,
    pub after: Option<V>,
}

impl<V> Change<V> {
    pub fn new(key: impl ToString, before: Option<V>, after: Option<V>) -> Self {
        Change {
            key: key.to_string(),
            before,
            after,
        }
    }
    pub fn unpack(self) -> (String, Option<V>, Option<V>) {
        (self.key, self.before, self.after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_functionality() {
        // Test construction and unpack
        let change = Change::new("test_key", Some("old"), Some("new"));
        let (key, before, after) = change.unpack();
        assert_eq!(key, "test_key");
        assert_eq!(before, Some("old"));
        assert_eq!(after, Some("new"));

        // Test different change scenarios
        let insert = Change::new("key1", None, Some(42));
        assert_eq!(insert.before, None);
        assert_eq!(insert.after, Some(42));

        let delete = Change::new("key2", Some("value"), None);
        assert_eq!(delete.before, Some("value"));
        assert_eq!(delete.after, None);
    }

    #[test]
    fn test_derived_traits() {
        let change1 = Change::new("key", Some(1), Some(2));
        let change2 = change1.clone();

        assert_eq!(change1, change2);
        assert_ne!(change1, Change::new("different", Some(1), Some(2)));
    }
}
