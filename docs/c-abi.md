# C ABI v1

The contract is [include/molite.h](../include/molite.h). Rust also produces `rlib` and `staticlib`; define `MOLITE_STATIC` before including the header when linking statically. See [examples/c_smoke.c](../examples/c_smoke.c).

Engine and Task are opaque `uint64_t` handles; zero is invalid. Do not dereference them. Buffers contain `{uint8_t *data; size_t len;}` without a NUL terminator.

1. `molite_open(path, len, &engine)` opens a UTF-8 data-directory path, not JSON.
2. `molite_submit(engine, json, len, &task)` copies the input and asynchronously executes the same operation JSON used by HTTP.
3. `molite_task_next(task, wait_ms, &buffer)` returns UTF-8 JSON events; each wait is capped at 1,000 ms.
4. Call `molite_buffer_free(buffer)` exactly once for every successfully returned buffer. Never use the host allocator to free it.
5. After `completed`, `failed` or `cancelled`, call `molite_task_release(task)`. Releasing a running task requests cancellation.
6. Release all tasks before `molite_close(engine)`; otherwise it returns BUSY.

Status codes: 0 OK, 1 EMPTY, 2 INVALID, 3 BUSY, 4 FAILED. Business errors appear in a failed event's `error` field. Handles are checked, but callers must still supply valid pointers, lengths and writable output storage. Cancellation does not roll back already committed exchanges; inspect history or durable events near a cancellation race.

Handles are process-local. Restart recovery uses persistent business state. Export boundaries catch Rust panics; host polling should still have a deadline.

On Windows run `cmd /c scripts\test-c-abi.cmd target\release`. On Linux/macOS run `bash scripts/test-c-abi.sh`. CI links the C host against each platform's dynamic library and runs it. UPX packaging tests the packed Windows DLL with the original import library as well as runtime operations.
