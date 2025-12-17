# retryx

Small, async-first retry & backoff helper for Tokio-based Rust services.

`retryx` gives you a fluent builder API to retry any async operation with fixed or exponential backoff, optional per-attempt timeouts, custom retry filters, and an `on_retry` hook for logging or metrics.

## Features

- **Async-first**: Designed for `async`/`await` and `tokio`.
- **Retry any async function**: Works with `Fn() -> Future<Output = Result<T, E>>`.
- **Delay strategies**: Fixed delay and exponential backoff with sane caps.
- **Per-attempt timeout**: Fail attempts that overrun, using `tokio::time::timeout`.
- **Custom retry filters**: Decide which errors are retryable.
- **`on_retry` hook**: Integrate logging and metrics.
- **No macros, no unsafe**: Simple, readable implementation.

## Quick start

Add this to your `Cargo.toml`:

```toml
[dependencies]
retryx = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

## Example: basic retry with exponential backoff

```rust
use retryx::retry;
use std::time::Duration;

#[derive(Debug, Clone)]
struct ApiError;

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "api error")
    }
}

impl std::error::Error for ApiError {}

impl From<tokio::time::error::Elapsed> for ApiError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        ApiError
    }
}

async fn call_api() -> Result<String, ApiError> {
    // Your real API call goes here
    Ok("ok".to_string())
}

#[tokio::main]
async fn main() -> Result<(), ApiError> {
    let value = retry::<ApiError>()
        .times(3)
        .exponential()
        .timeout(Duration::from_secs(2))
        .run(|| async {
            call_api().await
        })
        .await?;

    println!("Got value: {value}");
    Ok(())
}
```

## Example: custom filter and `on_retry` hook

```rust
use retryx::retry;
use std::time::Duration;

#[derive(Debug, Clone)]
enum ApiError {
    Timeout,
    RateLimited,
    Fatal,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Timeout => write!(f, "timeout"),
            ApiError::RateLimited => write!(f, "rate limited"),
            ApiError::Fatal => write!(f, "fatal error"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<tokio::time::error::Elapsed> for ApiError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        ApiError::Timeout
    }
}

async fn call_api() -> Result<String, ApiError> {
    // Your real API call goes here
    Err(ApiError::Timeout)
}

#[tokio::main]
async fn main() -> Result<(), ApiError> {
    let result = retry::<ApiError>()
        .times(5)
        .fixed(Duration::from_millis(200))
        .when(|err| matches!(err, ApiError::Timeout | ApiError::RateLimited))
        .on_retry(|attempt, err, delay| {
            println!("retry {attempt}: {err}, next in {delay:?}");
        })
        .run(|| async {
            call_api().await
        })
        .await;

    println!("Final result: {:?}", result);
    Ok(())
}
```

<!-- Published v1.0.0 -->


