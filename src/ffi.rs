//! Standard C ABI. IDs are validated opaque handles. All panics stop at the boundary.
//! Pointer validity and exclusive buffer ownership remain caller obligations.
use crate::runtime::Engine;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub const OK: i32 = 0;
pub const EMPTY: i32 = 1;
pub const INVALID: i32 = 2;
pub const BUSY: i32 = 3;
pub const FAILED: i32 = 4;
#[repr(C)]
pub struct Buffer {
    pub data: *mut u8,
    pub len: usize,
}
struct Task {
    owner: u64,
    receiver: std::sync::mpsc::Receiver<Value>,
    sender: std::sync::mpsc::Sender<Value>,
    abort: tokio::task::AbortHandle,
    terminal: Arc<AtomicBool>,
}
#[derive(Default)]
struct Handles {
    next: u64,
    engines: HashMap<u64, Engine>,
    tasks: HashMap<u64, Task>,
}
static HANDLES: OnceLock<Mutex<Handles>> = OnceLock::new();
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
fn handles() -> &'static Mutex<Handles> {
    HANDLES.get_or_init(|| Mutex::new(Handles::default()))
}
fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("create ABI runtime"))
}
fn boundary(f: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(FAILED)
}
unsafe fn input<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if ptr.is_null() || len > 90 * 1024 * 1024 {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(ptr, len) })
    }
}
unsafe fn output(ptr: *mut Buffer, value: Value) {
    let bytes = value.to_string().into_bytes().into_boxed_slice();
    let len = bytes.len();
    let data = Box::into_raw(bytes) as *mut u8;
    unsafe {
        ptr.write(Buffer { data, len });
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn molite_abi_version() -> u32 {
    1
}
/// # Safety
/// `path` points to `len` readable UTF-8 bytes; `out` is writable for one handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molite_open(path: *const u8, len: usize, out: *mut u64) -> i32 {
    boundary(|| {
        if out.is_null() {
            return INVALID;
        }
        unsafe {
            out.write(0);
        }
        let Some(bytes) = (unsafe { input(path, len) }) else {
            return INVALID;
        };
        let Ok(path) = std::str::from_utf8(bytes) else {
            return INVALID;
        };
        let _enter = runtime().enter();
        let Ok(engine) = Engine::open(path) else {
            return FAILED;
        };
        let Ok(mut h) = handles().lock() else {
            return FAILED;
        };
        h.next += 1;
        let id = h.next;
        h.engines.insert(id, engine);
        unsafe {
            out.write(id);
        }
        OK
    })
}
/// # Safety
/// `json_bytes` points to `len` readable bytes; `out` is writable for one handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molite_submit(
    engine: u64,
    json_bytes: *const u8,
    len: usize,
    out: *mut u64,
) -> i32 {
    boundary(|| {
        if out.is_null() {
            return INVALID;
        }
        unsafe {
            out.write(0);
        }
        let Some(bytes) = (unsafe { input(json_bytes, len) }) else {
            return INVALID;
        };
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            return INVALID;
        };
        let Ok(mut h) = handles().lock() else {
            return FAILED;
        };
        let Some(e) = h.engines.get(&engine).cloned() else {
            return INVALID;
        };
        if h.tasks.len() >= 1024 {
            return BUSY;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = tx.clone();
        let terminal = Arc::new(AtomicBool::new(false));
        let worker_terminal = terminal.clone();
        let task=runtime().spawn(async move{
  let(events,mut receiver)=tokio::sync::mpsc::unbounded_channel();
  let future=e.execute(value,Some(&events));tokio::pin!(future);
  let result=loop{tokio::select!{result=&mut future=>break result,Some(event)=receiver.recv()=>{let _=tx.send(event);}}};
  while let Ok(event)=receiver.try_recv(){let _=tx.send(event);}
  if !worker_terminal.swap(true,Ordering::SeqCst){let _=tx.send(match result{Ok(v)=>json!({"type":"completed","result":v}),Err(e)=>json!({"type":"failed","error":e.to_string()})});}
 });
        h.next += 1;
        let id = h.next;
        h.tasks.insert(
            id,
            Task {
                owner: engine,
                receiver: rx,
                sender,
                abort: task.abort_handle(),
                terminal,
            },
        );
        unsafe {
            out.write(id);
        }
        OK
    })
}
/// # Safety
/// `out` is writable for one buffer. Free each successful output exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molite_task_next(task: u64, wait_ms: u32, out: *mut Buffer) -> i32 {
    boundary(|| {
        if out.is_null() {
            return INVALID;
        }
        unsafe {
            out.write(Buffer {
                data: std::ptr::null_mut(),
                len: 0,
            });
        }
        let Ok(h) = handles().lock() else {
            return FAILED;
        };
        let Some(t) = h.tasks.get(&task) else {
            return INVALID;
        };
        // Bounded wait; caller can poll without preventing cancellation indefinitely.
        match t
            .receiver
            .recv_timeout(Duration::from_millis(wait_ms.min(1000) as u64))
        {
            Ok(v) => {
                unsafe {
                    output(out, v);
                }
                OK
            }
            Err(_) => EMPTY,
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molite_task_cancel(task: u64) -> i32 {
    boundary(|| {
        let Ok(h) = handles().lock() else {
            return FAILED;
        };
        let Some(t) = h.tasks.get(&task) else {
            return INVALID;
        };
        t.abort.abort();
        if !t.terminal.swap(true, Ordering::SeqCst) {
            let _ = t.sender.send(json!({"type":"cancelled"}));
        }
        OK
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molite_task_release(task: u64) -> i32 {
    boundary(|| {
        let Ok(mut h) = handles().lock() else {
            return FAILED;
        };
        let Some(t) = h.tasks.remove(&task) else {
            return INVALID;
        };
        t.abort.abort();
        OK
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molite_close(engine: u64) -> i32 {
    boundary(|| {
        let Ok(mut h) = handles().lock() else {
            return FAILED;
        };
        if h.tasks.values().any(|t| t.owner == engine) {
            return BUSY;
        }
        if h.engines.remove(&engine).is_some() {
            OK
        } else {
            INVALID
        }
    })
}
/// # Safety
/// `buffer` is a live buffer returned by this library, never freed previously.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molite_buffer_free(buffer: Buffer) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !buffer.data.is_null() {
            unsafe {
                drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                    buffer.data,
                    buffer.len,
                )));
            }
        }
    }));
}
