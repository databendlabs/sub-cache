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

use crate::errors::either::Either;

/// The connection to the remote data store has been closed.
///
/// This error is used to represent various types of connection failures:
/// - Network errors (IO errors)
/// - Protocol errors (gRPC status errors)
/// - Stream closure
/// - Other connection-related issues
///
/// The error includes:
/// - The reason for the connection closure
/// - A chain of contexts describing when the error occurred
///
/// # Usage
///
/// ```rust
/// # use sub_cache::errors::ConnectionClosed;
/// let err = ConnectionClosed::new_str("connection reset")
///     .context("establishing watch stream")
///     .context("initializing cache");
/// ```
#[derive(thiserror::Error, Debug)]
pub struct ConnectionClosed {
    /// The reason for the connection closure.
    /// Can be either an IO error or a string description.
    reason: Either<io::Error, String>,

    /// A chain of contexts describing when the error occurred.
    /// Each context is added using the `context` method.
    when: Vec<String>,
}

impl ConnectionClosed {
    /// Create a new connection closed error.
    ///
    /// # Parameters
    ///
    /// * `reason` - The reason for the connection closure.
    ///   Can be either an IO error or a string description.
    pub fn new(reason: impl Into<Either<io::Error, String>>) -> Self {
        ConnectionClosed {
            reason: reason.into(),
            when: vec![],
        }
    }

    /// Create a new connection closed error from an io::Error.
    ///
    /// # Parameters
    ///
    /// * `reason` - The IO error that caused the connection closure.
    pub fn new_io_error(reason: impl Into<io::Error>) -> Self {
        ConnectionClosed {
            reason: Either::A(reason.into()),
            when: vec![],
        }
    }

    /// Create a new connection closed error from a string.
    ///
    /// # Parameters
    ///
    /// * `reason` - A string description of why the connection was closed.
    pub fn new_str(reason: impl ToString) -> Self {
        ConnectionClosed {
            reason: Either::B(reason.to_string()),
            when: vec![],
        }
    }

    /// Append a context to the error.
    ///
    /// This method can be used to build a chain of contexts describing
    /// when the error occurred.
    ///
    /// # Parameters
    ///
    /// * `context` - A string describing when the error occurred.
    ///
    /// # Returns
    ///
    /// The error with the new context appended.
    pub fn context(mut self, context: impl ToString) -> Self {
        self.when.push(context.to_string());
        self
    }
}

impl fmt::Display for ConnectionClosed {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "distributed-cache connection closed: {}", self.reason)?;

        if self.when.is_empty() {
            return Ok(());
        }

        write!(f, "; when: (")?;

        for (i, when) in self.when.iter().enumerate() {
            if i > 0 {
                write!(f, "; ")?;
            }
            write!(f, "{}", when)?;
        }

        write!(f, ")")
    }
}

impl From<io::Error> for ConnectionClosed {
    fn from(err: io::Error) -> Self {
        ConnectionClosed::new_io_error(err)
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn test_string_error_creation() {
        let err = ConnectionClosed::new_str("connection failed");
        assert_eq!(
            err.to_string(),
            "distributed-cache connection closed: connection failed"
        );
    }

    #[test]
    fn test_io_error_creation_and_from_trait() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused");
        let err: ConnectionClosed = io_err.into();
        assert_eq!(
            err.to_string(),
            "distributed-cache connection closed: connection refused"
        );
    }

    #[test]
    fn test_context_functionality() {
        let err = ConnectionClosed::new_str("base error")
            .context("step1")
            .context("step2")
            .context("step3");
        assert_eq!(
            err.to_string(),
            "distributed-cache connection closed: base error; when: (step1; step2; step3)"
        );
    }

    #[test]
    fn test_context_with_io_error() {
        let io_err = io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF");
        let err = ConnectionClosed::new_io_error(io_err)
            .context("reading stream")
            .context("processing events");
        assert_eq!(
            err.to_string(),
            "distributed-cache connection closed: unexpected EOF; when: (reading stream; processing events)"
        );
    }

    #[test]
    fn test_edge_cases() {
        // Empty string
        let err1 = ConnectionClosed::new_str("");
        assert_eq!(err1.to_string(), "distributed-cache connection closed: ");

        // Special characters in context
        let err2 = ConnectionClosed::new_str("error")
            .context("step with; semicolon")
            .context("step with ) parenthesis");
        assert_eq!(
            err2.to_string(),
            "distributed-cache connection closed: error; when: (step with; semicolon; step with ) parenthesis)"
        );
    }
}
