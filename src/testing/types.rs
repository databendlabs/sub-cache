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

use crate::testing::source::TestSource;
use crate::testing::source::Val;
use crate::TypeConfig;

#[derive(Debug, Default)]
pub struct TestConfig;

impl TypeConfig for TestConfig {
    type Value = Val;
    type Source = TestSource;

    fn value_seq(value: &Self::Value) -> u64 {
        value.seq
    }

    fn spawn<F>(future: F, _name: impl ToString)
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        // For testing, we need to spawn the future
        // This works in both unit tests and integration tests when tokio rt is available
        tokio::spawn(future);
    }
}
