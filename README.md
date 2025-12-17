# retryx

[![Crates.io](https://img.shields.io/crates/v/retryx.svg)](https://crates.io/crates/retryx)
[![Docs.rs](https://docs.rs/retryx/badge.svg)](https://docs.rs/retryx)
[![License](https://img.shields.io/crates/l/retryx.svg)](https://crates.io/crates/retryx)

Small, async-first **retry & backoff** helper for Tokio-based Rust services.

`retryx` gives you a tiny, fluent builder to wrap any async operation that returns `Result<T, E>` and retry it with fixed or exponential backoff, optional per-attempt timeouts, custom retry filters, and an `on_retry` hook for logging or metrics.

## Features

- **Async-first**: Built for `async`/`await` and `tokio`.
- **Retry any async function**: Works with `Fn() -> Future<Output = Result<T, E>>`.
- **Delay strategies**: Fixed delay and exponential backoff with sensible caps.
- **Per-attempt timeout**: Wraps each attempt in `tokio::time::timeout`.
- **Custom retry filters**: Decide which errors are retryable.
- **`on_retry` hook**: Integrate logging and metrics without extra plumbing.
- **No macros, no unsafe**: Simple, readable implementation.

## Why retryx?

Most retry helpers either come with a lot of policy surface area or hide behavior behind macros; `retryx` keeps the core primitive tiny and explicit, so you can see and control every retry, delay, and timeout in one fluent builder call.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
retryx = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
```

## Guarantees

- **No panics**: The crate does not intentionally panic under normal operation.
- **No global state**: All configuration lives in the `Retry` builder you create.
- **No background tasks**: Retries run in the caller’s async context; no hidden workers or spawned tasks.

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
