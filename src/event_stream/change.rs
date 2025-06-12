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
