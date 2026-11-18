# 集成测试规划

## 目标

验证分布式缓存系统的正确性、健壮性和一致性，覆盖多组件协作、并发、异常恢复等真实业务场景。

## 已实现的集成测试

### 1. 基础功能测试

#### `basic_sync.rs` - 基本同步测试
验证缓存与数据源的基本同步机制：启动 watcher，数据源变更，验证本地 cache 自动同步。

#### `cache_api.rs` - 缓存 API 测试
验证 Cache 结构体的所有公共 API 方法：`try_get()`, `try_last_seq()`, `try_list_dir()`, `try_access()` 等。

### 2. 并发与多客户端协作测试

#### `concurrent_same_range.rs` - 同范围并发监听测试
验证多个缓存实例监听相同键范围时的一致性。3个缓存实例订阅相同范围，并发数据更新。
- 使用 `BatchConfig` 结构体统一管理测试配置

#### `multi_watcher_concurrent.rs` - 多监听器并发协作测试
验证多个缓存实例监听重叠键范围时的协作。测试范围隔离和跨缓存一致性。

#### `concurrent_write.rs` - 持续写入一致性测试
验证持续写入场景下的缓存一致性。后台任务持续写入，缓存保持实时一致性。

### 3. 异常与恢复测试

#### `recovery.rs` - 连接恢复测试
验证连接失败后的自动恢复能力。模拟网络中断，验证自动重连和状态恢复。

#### `failed_initial_flush.rs` - 初始数据中断测试
验证初始数据（initial flush）中途断开时，读者读不到其中一部分：首次初始化时得到错误，重连时得到重连前的完整数据。

#### `unsupported_subscribe.rs` - 不支持订阅测试
验证数据源不支持订阅时的优雅降级。返回 `SubscribeError::Unsupported` 错误。

#### `drop_cache.rs` - 丢弃缓存测试
验证丢弃 `Cache` 后其 watcher 随即退出并释放 `Source`，包括 watcher 仍在建立缓存时：数据源不可达、不支持订阅，或初始数据（initial flush）未完成。

### 4. 事件处理与一致性测试

#### `event_ordering.rs` - 事件排序和幂等性测试
验证乱序和重复事件下的缓存正确性。基于序列号的事件处理和幂等性。

## 测试架构特点

### 配置化测试设计
- **`BatchConfig`**: 统一的批次配置结构 (`start`, `span`, `update_delay_ms`, `range()`)
- **`TestConfiguration`**: 完整的测试配置结构

### 工具函数支持
- `wait_for_cache_state()`, `check_cache_state()`, `retry_check_cache_state()`
- `TestSource`: 模拟数据源，支持连接管理

### 测试覆盖矩阵

| 测试场景 | 基础同步 | API功能 | 并发协作 | 异常恢复 | 事件处理 |
|---------|---------|---------|---------|---------|---------|
| 数据一致性 | ✓ | ✓ | ✓ | ✓ | ✓ |
| 并发安全 | - | - | ✓ | - | - |
| 错误处理 | - | ✓ | - | ✓ | - |
| 序列一致性 | ✓ | ✓ | ✓ | ✓ | ✓ |
| 资源管理 | - | - | - | ✓ | - |
| 边界情况 | - | ✓ | - | - | ✓ |

## 测试运行

```bash
cargo test --test <test_name>  # 运行单个测试
cargo test --tests            # 运行所有集成测试
``` 