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

use std::fmt;
use std::io;

/// Either is a type that can be either one of two types.
///
/// This is used to represent errors that can be either an `io::Error` or a `String`.
/// It provides a convenient way to handle different types of error sources
/// in a unified way.
///
/// # Examples
///
/// ```rust
/// use std::io;
///
/// use sub_cache::errors::Either;
///
/// let err: Either<io::Error, String> =
///     Either::A(io::Error::new(io::ErrorKind::Other, "network error"));
/// let err: Either<io::Error, String> = Either::B("protocol error".to_string());
/// ```
#[derive(Debug)]
pub enum Either<A, B> {
    /// The first possible type.
    A(A),
    /// The second possible type.
    B(B),
}

impl fmt::Display for Either<io::Error, String> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Either::A(e) => write!(f, "{}", e),
            Either::B(s) => write!(f, "{}", s),
        }
    }
}

impl From<io::Error> for Either<io::Error, String> {
    fn from(e: io::Error) -> Self {
        Either::A(e)
    }
}

impl From<String> for Either<io::Error, String> {
    fn from(s: String) -> Self {
        Either::B(s)
    }
}

impl From<&str> for Either<io::Error, String> {
    fn from(s: &str) -> Self {
        Either::B(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn test_enum_variants() {
        let io_err = io::Error::other("test error");
        let either_a: Either<io::Error, String> = Either::A(io_err);
        let either_b: Either<io::Error, String> = Either::B("string error".to_string());

        match either_a {
            Either::A(_) => {}
            Either::B(_) => panic!("Expected Either::A"),
        }

        match either_b {
            Either::A(_) => panic!("Expected Either::B"),
            Either::B(s) => assert_eq!(s, "string error"),
        }
    }

    #[test]
    fn test_display_implementation() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused");
        let either_io: Either<io::Error, String> = Either::A(io_err);
        assert_eq!(either_io.to_string(), "connection refused");

        let either_string: Either<io::Error, String> = Either::B("custom error".to_string());
        assert_eq!(either_string.to_string(), "custom error");
    }

    #[test]
    fn test_from_io_error() {
        let io_err = io::Error::new(io::ErrorKind::TimedOut, "operation timed out");
        let either: Either<io::Error, String> = io_err.into();

        match either {
            Either::A(e) => assert_eq!(e.to_string(), "operation timed out"),
            Either::B(_) => panic!("Expected Either::A"),
        }
    }

    #[test]
    fn test_from_string() {
        let error_msg = "protocol error".to_string();
        let either: Either<io::Error, String> = error_msg.into();

        match either {
            Either::A(_) => panic!("Expected Either::B"),
            Either::B(s) => assert_eq!(s, "protocol error"),
        }
    }

    #[test]
    fn test_from_str() {
        let either: Either<io::Error, String> = "string literal error".into();

        match either {
            Either::A(_) => panic!("Expected Either::B"),
            Either::B(s) => assert_eq!(s, "string literal error"),
        }
    }

    #[test]
    fn test_string_variants_only() {
        // Test only string variants since io::Error doesn't implement Clone/PartialEq
        let either1: Either<io::Error, String> = Either::B("test".to_string());
        let either2: Either<io::Error, String> = Either::B("test".to_string());

        // We can only test the content, not equality due to io::Error limitations
        match (&either1, &either2) {
            (Either::B(s1), Either::B(s2)) => assert_eq!(s1, s2),
            _ => panic!("Expected both to be Either::B"),
        }
    }

    #[test]
    fn test_debug_formatting() {
        let either_string: Either<io::Error, String> = Either::B("debug test".to_string());
        let debug_output = format!("{:?}", either_string);
        assert!(debug_output.contains("debug test"));
        assert!(debug_output.contains("B("));
    }
}
