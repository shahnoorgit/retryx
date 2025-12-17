//! Async-first retry & backoff helper for Tokio-based Rust services.
//!
//! Published v1.0.0
//!
//! This crate provides a small, opinionated `retry()` builder that can wrap any
//! async operation returning `Result<T, E>` and retry it with fixed or
//! exponential backoff.
//!
//! Minimal example:
//!
//! ```rust,no_run
//! use retryx::retry;
//! use std::time::Duration;
//!
//! #[derive(Debug, Clone)]
//! struct MyError;
//!
//! impl std::fmt::Display for MyError {
//!     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//!         write!(f, "my error")
//!     }
//! }
//!
//! impl std::error::Error for MyError {}
//!
//! impl From<tokio::time::error::Elapsed> for MyError {
//!     fn from(_: tokio::time::error::Elapsed) -> Self {
//!         MyError
//!     }
//! }
//!
//! async fn call_api() -> Result<String, MyError> {
//!     // Your real API call goes here.
//!     Ok("ok".to_string())
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), MyError> {
//!     let value = retry::<MyError>()
//!         .times(3)
//!         .exponential()
//!         .timeout(Duration::from_secs(2))
//!         .run(|| async {
//!             call_api().await
//!         })
//!         .await?;
//!
//!     println!("Got value: {value}");
//!     Ok(())
//! }
//! ```

#![forbid(unsafe_code)]

use std::future::Future;
use std::time::Duration;

use tokio::time::{sleep, timeout};

/// Maximum backoff delay to prevent unbounded waits.
const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(30);

/// Default initial delay for backoff strategies.
const DEFAULT_INITIAL_DELAY: Duration = Duration::from_millis(100);

/// Delay strategy used between retry attempts.
#[derive(Clone, Copy)]
enum DelayStrategy {
    /// Fixed delay between attempts.
    Fixed { delay: Duration },
    /// Exponential backoff based on attempt number.
    Exponential {
        base_delay: Duration,
        max_delay: Duration,
        factor: f64,
    },
}

/// Builder for configuring and running async retries.
///
/// Construct via [`retry()`].
#[must_use = "retry builder does nothing unless `.run()` is called"]
pub struct Retry<E> {
    max_attempts: usize,
    strategy: DelayStrategy,
    timeout_per_attempt: Option<Duration>,
    /// Optional predicate to decide whether to retry on a given error.
    condition: Option<Box<dyn Fn(&E) -> bool + Send + Sync>>,
    /// Optional hook invoked before each retry.
    on_retry: Option<Box<dyn Fn(usize, &E, Duration) + Send + Sync>>,
}

impl<E> Retry<E> {
    /// Create a new retry builder with sane defaults.
    ///
    /// Defaults:
    /// - 3 attempts
    /// - Exponential backoff starting at 100ms, capped at 30s
    /// - No per-attempt timeout
    /// - Always retry on error
    /// - No-op `on_retry` hook
    pub fn new() -> Self {
        Self {
            max_attempts: 3,
            strategy: DelayStrategy::Exponential {
                base_delay: DEFAULT_INITIAL_DELAY,
                max_delay: DEFAULT_MAX_DELAY,
                factor: 2.0,
            },
            timeout_per_attempt: None,
            condition: None,
            on_retry: None,
        }
    }

    /// Set maximum number of attempts (including the first call).
    pub fn times(mut self, attempts: usize) -> Self {
        self.max_attempts = attempts.max(1);
        self
    }

    /// Use a fixed delay between attempts.
    pub fn fixed(mut self, delay: Duration) -> Self {
        self.strategy = DelayStrategy::Fixed { delay };
        self
    }

    /// Use exponential backoff between attempts.
    ///
    /// Starts at 100ms and doubles each time, capped at 30s.
    pub fn exponential(mut self) -> Self {
        self.strategy = DelayStrategy::Exponential {
            base_delay: DEFAULT_INITIAL_DELAY,
            max_delay: DEFAULT_MAX_DELAY,
            factor: 2.0,
        };
        self
    }

    /// Set a per-attempt timeout.
    ///
    /// If the operation exceeds this duration, the attempt fails with
    /// `tokio::time::error::Elapsed` converted into the error type `E`
    /// via `E: From<tokio::time::error::Elapsed>`.
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.timeout_per_attempt = Some(duration);
        self
    }

    /// Customize the retry condition.
    ///
    /// The provided closure receives the error from an attempt.
    /// Return `true` to retry, `false` to stop and return the error.
    pub fn when<F>(mut self, condition: F) -> Self
    where
        F: Fn(&E) -> bool + Send + Sync + 'static,
    {
        self.condition = Some(Box::new(condition));
        self
    }

    /// Register an `on_retry` hook for logging or metrics.
    ///
    /// The hook receives:
    /// - attempt number (1-based)
    /// - the error from the last attempt
    /// - the delay before the next attempt
    pub fn on_retry<F>(mut self, hook: F) -> Self
    where
        F: Fn(usize, &E, Duration) + Send + Sync + 'static,
    {
        self.on_retry = Some(Box::new(hook));
        self
    }

    /// Run the provided async operation with the configured retry policy.
    pub async fn run<Op, Fut, T>(self, mut op: Op) -> Result<T, E>
    where
        Op: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: From<tokio::time::error::Elapsed>,
    {
        // Attempts are counted as 1-based: the very first execution is attempt 1.
        let mut attempt = 0usize;

        loop {
            attempt += 1;

            // Run the operation, optionally wrapped in a per-attempt timeout.
            let result = if let Some(dur) = self.timeout_per_attempt {
                match timeout(dur, op()).await {
                    Ok(res) => res,
                    Err(elapsed) => Err(E::from(elapsed)),
                }
            } else {
                op().await
            };

            match result {
                Ok(value) => return Ok(value),
                Err(err) => {
                    let should_retry = attempt < self.max_attempts
                        && self
                            .condition
                            .as_ref()
                            // Default: always retry if no condition is configured.
                            .map(|f| f(&err))
                            .unwrap_or(true);

                    if !should_retry {
                        return Err(err);
                    }

                    let delay = self.next_delay(attempt);

                    // Fire the on_retry hook before sleeping so callers can log/record metrics.
                    // The attempt number given to the hook is also 1-based.
                    if let Some(ref hook) = self.on_retry {
                        hook(attempt, &err, delay);
                    }

                    sleep(delay).await;
                }
            }
        }
    }

    /// Compute the delay before the given attempt (1-based).
    ///
    /// The `attempt` argument matches the 1-based attempt number exposed to users,
    /// so `attempt == 1` corresponds to the first call to the operation.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn next_delay(&self, attempt: usize) -> Duration {
        match self.strategy {
            // Fixed delay is straightforward: we always sleep for the configured duration.
            DelayStrategy::Fixed { delay } => delay,
            DelayStrategy::Exponential {
                base_delay,
                max_delay,
                factor,
            } => {
                // `attempt` is 1-based: attempt 1 uses `base_delay`,
                // attempt 2 uses `base_delay * factor`, and so on.
                //
                // Floating-point math is safe here because:
                // - the exponent is small (bounded by `max_attempts`), so precision loss is tiny,
                // - we clamp the final result to `max_delay`,
                // - and `Duration` itself has much coarser granularity than f64 can represent.
                let exp = (attempt.saturating_sub(1)) as u32;
                let multiplier = factor.powi(exp as i32);
                let nanos =
                    (base_delay.as_nanos() as f64 * multiplier).min(max_delay.as_nanos() as f64);
                Duration::from_nanos(nanos as u64)
            }
        }
    }
}

/// Create a new retry builder with sane defaults.
///
/// Type parameter `E` (the error type) is inferred from the async
/// operation you pass to [`Retry::run`].
pub fn retry<E>() -> Retry<E> {
    Retry::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Debug, Clone)]
    struct MyError;

    impl std::fmt::Display for MyError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "my error")
        }
    }

    impl std::error::Error for MyError {}

    impl From<tokio::time::error::Elapsed> for MyError {
        fn from(_: tokio::time::error::Elapsed) -> Self {
            MyError
        }
    }

    #[tokio::test]
    async fn retries_until_success() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let result: usize = retry::<MyError>()
            .times(3)
            .fixed(Duration::from_millis(1))
            .run(move || {
                let counter = counter_clone.clone();
                async move {
                    let current = counter.fetch_add(1, Ordering::SeqCst);
                    if current < 2 {
                        Err(MyError)
                    } else {
                        Ok(current)
                    }
                }
            })
            .await
            .unwrap();

        assert_eq!(result, 2);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn obeys_when_filter() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        #[derive(Debug, Clone)]
        enum ApiError {
            Timeout,
            Fatal,
        }

        impl std::fmt::Display for ApiError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    ApiError::Timeout => write!(f, "timeout"),
                    ApiError::Fatal => write!(f, "fatal"),
                }
            }
        }

        impl std::error::Error for ApiError {}

        impl From<tokio::time::error::Elapsed> for ApiError {
            fn from(_: tokio::time::error::Elapsed) -> Self {
                ApiError::Timeout
            }
        }

        let err = retry::<ApiError>()
            .times(5)
            .fixed(Duration::from_millis(1))
            .when(|err| matches!(err, ApiError::Timeout))
            .run(move || {
                let counter = counter_clone.clone();
                async move {
                    let n = counter.fetch_add(1, Ordering::SeqCst);
                    if n == 0 {
                        Err::<(), ApiError>(ApiError::Timeout)
                    } else {
                        Err::<(), ApiError>(ApiError::Fatal)
                    }
                }
            })            
            .await
            .unwrap_err();

        // First error was retried, second was not.
        assert!(matches!(err, ApiError::Fatal));
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}



