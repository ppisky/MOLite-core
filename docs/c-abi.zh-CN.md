# C ABI v1

[English](c-abi.md)

实际合同为 [`include/molite.h`](../include/molite.h)，测试脚本为 `scripts/test-c-abi.cmd`。当前实机验证 Windows x64；同时生成 rlib、cdylib、staticlib。

Android 宿主须在 JVM 线程上先调用 `molite_android_init(JNIEnv*, ApplicationContext)`，再调用 `molite_open`。
该 Android 专用扩展初始化 rustls 的系统证书验证器；必须同时打包与 Cargo.lock 中
`rustls-platform-verifier-android` 版本一致的 `org.rustls:rustls-platform-verifier` AAR（当前 0.2.0）。
Context 会被转为全局引用，只接受 Application Context，不能传 Activity。
返回既有状态码，不改变 C ABI v1 的任务与缓冲区合同。其他平台不导出此扩展。
使用 NDK r28 以上构建 arm64-v8a/x86_64 `.so`，并设置 16 KiB ELF 页对齐。

Engine 和 Task 为 `uint64_t` 不透明句柄，0无效，不能解引用。公共缓冲区为 `{uint8_t *data; size_t len;}`，不带 NUL。

1. `molite_open(path, len, &engine)`：UTF-8数据目录，不是 JSON。
2. `molite_submit(engine, json, len, &task)`：复制输入并异步执行，与 HTTP 相同的操作 JSON。
3. `molite_task_next(task, wait_ms, &buffer)`：拉取 UTF-8 JSON 事件，单次最多等待1000ms。
4. 每个成功返回的缓冲区恰好调用一次 `molite_buffer_free(buffer)`；不能用宿主 free 或重复释放。
5. 收到 completed、failed、cancelled 后 `molite_task_release(task)`。释放运行中任务会请求取消。
6. 释放所有任务后 `molite_close(engine)`；否则返回 BUSY。

状态码：0 OK、1 EMPTY、2 INVALID、3 BUSY、4 FAILED。业务错误在 failed 事件的 error 字段。边界检查句柄，调用方仍须保证指针、长度和写入空间合法。取消不会回滚已经提交的交换；取消附近需查询历史/持久事件确认落盘状态。

任务句柄只在当前进程有效，重启按业务持久化状态恢复。导出函数拦截 panic；宿主仍应为任务轮询设置超时。历史方案见 [早期草案](c-abi-proposal.zh-CN.md)，不作为当前 ABI 合同。
