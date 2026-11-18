# Distributed Cache

A distributed cache implementation providing reliable resource management and data synchronization across distributed systems.

## Features

- **Automatic Synchronization**: Background watcher task keeps local cache in sync with remote data store
- **Concurrency Control**: Two-level concurrency control mechanism for safe access
- **Event-based Updates**: Real-time updates through watch API
- **Safe Reconnection**: Automatic recovery from connection failures with state consistency

## Key Components

### Cache Structure

```text
<dir>/foo
<dir>/..
<dir>/..
```

- `<dir>`: The cached directory, used as given: the cache holds the keys under `<dir>/`, and
  `foo/` is the empty-named directory inside `foo`, whose keys are `foo//...`. It also
  identifies a cache instance

### Main Types

- `Cache`: The main entry point for cache operations
  - Provides safe access to cached data
- `CacheData`: Internal data structure holding the cached values
- `EventWatcher`: Background task that watches for changes in the remote data store
  - Handles synchronization with remote data store

## Usage

```rust
let client = RemoteClient::try_create(/*..*/);
let cache = Cache::new(
        client,
        "your/cache/key/space",
        "your-app-name-for-logging",
).await;

// Access cached data
cache.try_access(|c: &CacheData| {
    println!("last-seq:{}", c.last_seq);
    println!("all data: {:?}", c.data);
}).await?;

// Get a specific value
let value = cache.try_get("key").await?;

// List all entries under a directory
let entries = cache.try_list_dir("dir").await?;

// Read without waiting: `None` while the cache initializes, reconnects or applies a change
if let Some(value) = cache.try_get_now("key") {
    let value = value?;
}
```

## Concurrency Control

The data is behind a read-write lock. Readers share it; the background watcher holds it exclusively, briefly to apply each change, and for the whole reset-and-refetch while it initializes or reconnects.

Every read takes `&self`, so a cache is shared by reference or `Arc` without an outer lock:

- `try_get`, `try_list_dir`, `try_last_seq` and `try_access` wait while the watcher holds the data, e.g. until the cache is initialized.
- `try_get_now`, `try_list_dir_now` and `try_access_now` never wait: they return `None` while the watcher holds the data, and the caller reads the remote data store instead. They are not futures, so they answer whether or not the task's tokio coop budget is spent.

## Initialization Process

When a `Cache` is created, it goes through the following steps:

1. Creates a new instance of the specified directory and name
2. Spawns a background task to watch for key-value changes
3. Establishes a watch stream to the remote data store
4. Fetches and processes initial data
5. Waits for the cache to be fully initialized before returning
6. Maintains continuous synchronization

The initialization is complete only when the cache has received a full copy of the data from the remote data store, ensuring users see a consistent view of the data.

## Error Handling

The cache implements robust error handling:

- Connection failures are automatically retried in the background
- Background watcher task automatically recovers from errors
- Users are shielded from transient errors through the abstraction
- The cache ensures data consistency by tracking sequence numbers

## License

This project is licensed under the Apache License 2.0 - see the LICENSE file for details.
