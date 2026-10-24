# MOLite Core C ABI 设计

状态：接口设计草案，尚无动态库、已发布 ABI 或可调用实现。日期：2026-10-24。

## 1. 必备交付与边界

提供 `molite.h` 与目标平台动态库（Windows DLL、Linux SO、macOS dylib）。静态库可后续按宿主需求增加。使用平台 C 调用约定、C 链接符号和稳定的导出名称，C++ 宿主通过 `extern "C"` 声明调用。

ABI 稳定性针对明确的操作系统、架构及工具链目标，不代表不同架构共享二进制布局。发布时明确目标矩阵；首个需要验证的本地目标为 Windows x64。

公共头文件不出现 Rust 类型、泛型、trait、`String`、`Vec`、引用或 future。实现导出函数使用 `extern "C"`，跨边界结构使用 C 兼容布局；相关约束依据 [Rust FFI 指南](https://doc.rust-lang.org/nomicon/ffi.html) 和 [Rust 类型布局规则](https://doc.rust-lang.org/reference/type-layout.html)。

## 2. 建议接口形态

优先采用异步任务与事件拉取，避免首版承担异步回调的线程、重入和宿主对象生命周期。下列为可评审的头文件草案，不代表名称或 wire 已冻结：

```c
#ifndef MOLITE_H
#define MOLITE_H

#include <stddef.h>
#include <stdint.h>

/* MOLITE_API 按构建目标定义为 DLL 导入/导出或符号可见性标记。 */
#ifndef MOLITE_API
#define MOLITE_API
#endif

/* Windows 32 位若列入支持范围，必须显式固定 cdecl。 */
#if defined(_WIN32)
#define MOLITE_CALL __cdecl
#else
#define MOLITE_CALL
#endif

#ifdef __cplusplus
extern "C" {
#endif

typedef struct molite_engine molite_engine;
typedef struct molite_task molite_task;
typedef int32_t molite_status;

typedef struct molite_buffer {
    uint8_t *data;
    size_t len;
} molite_buffer;

MOLITE_API uint32_t MOLITE_CALL molite_abi_version(void);

MOLITE_API molite_status MOLITE_CALL molite_open(
    const uint8_t *options_json, size_t options_len,
    molite_engine **out_engine, molite_buffer *out_error);

MOLITE_API molite_status MOLITE_CALL molite_submit(
    molite_engine *engine,
    const uint8_t *operation_json, size_t operation_len,
    molite_task **out_task, molite_buffer *out_error);

MOLITE_API molite_status MOLITE_CALL molite_task_next(
    molite_task *task, uint32_t timeout_ms,
    molite_buffer *out_event, molite_buffer *out_error);

MOLITE_API molite_status MOLITE_CALL molite_task_cancel(
    molite_task *task, molite_buffer *out_error);

MOLITE_API molite_status MOLITE_CALL molite_task_release(
    molite_task *task, molite_buffer *out_error);

MOLITE_API molite_status MOLITE_CALL molite_close(
    molite_engine *engine, molite_buffer *out_error);

MOLITE_API void MOLITE_CALL molite_buffer_free(molite_buffer *buffer);

#ifdef __cplusplus
}
#endif
#endif
```

生命周期接口固定在 ABI；业务操作使用带 schema 版本的 UTF-8 JSON envelope，在入口立即解析为内部类型。该选择让首版不必为每个业务字段固定 C 结构布局，也便于 C#、Python 等宿主绑定。JSON 是真实跨语言边界，内部不通过反复 JSON 转换串联模块。

操作集合固定并有版本化 schema，至少覆盖 MOC 导入导出、原生角色、会话及当前叙事状态、记忆注入范围、文件夹标签、数据文件、服务商与模型、模型参数组、响应、来源修复及诊断；不接受任意插件操作或直接 SQL。场景与会话的最终命名、映射尚未冻结。模型参数组定义模型、请求方式和参数，三个用途分别引用组；不是一个 C 结构内固定三套用途配置。MOC 二进制建议先通过路径导入导出，路径通过 UTF-8 字段传递，Windows 内部转换到系统路径表示；无需把容器转成 JSON/base64。内存缓冲导入后续有具体需求再设计。

响应操作支持可选图片输入，按基础服务商下所选模型的能力选择原生多模态请求或视觉回退转文本。建议首版以宿主提供的本地图片路径接入：任务接受阶段完成受限读取、校验及私有暂存快照，之后只使用该快照，不让宿主后续替换文件改变已接受输入；接受过程允许有界磁盘 I/O。图片路径不能当成任务整个生命周期都有效的借用数据。图片处理路径、回退转换结果、缺少必需视觉配置及上游失败通过同一任务事件与错误契约返回，不引入另一套 ABI 或图片载体接口。

数据文件不限于图片，至少涵盖用户明确提出的 TXT、PNG、PDF 存储。建议提供文件注册/导入任务，返回稳定文件 ID，后续消息引用 ID；本地路径是输入来源，不是文件的永久身份。文件内容、哈希、消息证据和 MOC 携带/排除选择遵循 Core 同一套规则；成功存储文件不代表模型已经读取其内容。

编辑、重新生成或删除消息的操作返回受影响来源和修复任务信息，撤下失效记忆后重新提炼；调用方可读取处理中、完成、失败/待重试的状态并通知使用者。正常修复无需再次批准，也不等待 12 个新问答。

## 3. 所有权与错误

- 输入是调用期间有效的借用字节；`submit` 返回前复制异步任务需要的内容。
- `out_engine`、`out_task`、输出缓冲指针必须有效。失败时句柄输出置空；每次调用输出缓冲前调用者应提供空结构。不能将尚未释放的缓冲当作新的输出目标。
- `len == 0` 的输入允许空指针；非零长度必须对应有效可读内存。无法安全校验任意外来地址，错误指针属于调用方违反 ABI 合约。
- 输出缓冲由库分配，只能通过 `molite_buffer_free` 释放，不得由宿主 `free/delete`。缓冲为明确长度的字节，不保证末尾 NUL；JSON 是 UTF-8，不能按系统 ANSI 编码读取。
- `molite_buffer_free` 释放后将该结构置空；缓冲不能复制后重复释放。任务或 engine 释放不影响调用方已持有、尚未释放的独立输出缓冲。
- 状态码为固定宽度整数，区别成功、超时无事件、任务已结束、参数错误、冲突、忙碌、取消、上游失败及内部错误；具体数值在头文件冻结时分配。
- 错误通过本次调用的 `out_error` 返回，不使用跨线程共享的 `last_error`。错误包含稳定机器码和可读消息，不泄露模型凭据。

## 4. 并发、事件与关闭

Core 自持有执行器，宿主只负责提交和读取事件。同一 engine 可从不同线程提交；同一 task 同时最多一个 `next`，允许另一线程调用取消。句柄释放或 engine 关闭不得与使用同一对象的调用并发。

`next(timeout_ms=0)` 非阻塞，正值为有界等待。输出事件拥有任务内递增序号，并区分文本增量、完成、失败和取消终态。成功完成事件只在助手结果及相应待办持久提交后发出。流式片段是暂定内容，不能将最后一段文本当作成功标记。

事件队列必须有容量和字节上限；宿主长期不读取时通过有界等待或明确取消处理，不能无限积累 token。终态到达后允许宿主读完剩余事件；队列耗尽返回已结束。

取消是请求终止，不撤销已经原子提交的结果；与完成竞态时由已提交终态决定。`task_release` 在任务运行中返回忙碌，调用者应先取消并等待终态；成功释放后句柄失效。

`close` 在还有未释放任务句柄时返回忙碌，成功前不销毁 engine。全部任务释放后，关闭过程停止后台工作、持久保存恢复点并等待线程退出；失败时保留有效 engine 供诊断或重试。关闭耗时应有启动选项中的明确上限，不能隐式等待远程模型无限返回。

## 5. 版本与异常边界

ABI 版本与业务 JSON schema 分别版本化。宿主加载后先查询 ABI 主版本，不兼容时在创建实例前停止。公开结构一旦冻结不得重排或改变字段含义；新结构采用独立版本或明确的长度协商。

Rust panic 不得穿过 C ABI。可捕获的 unwind 在边界转成内部错误，后台任务 panic 也必须进入可观察的失败状态；涉及共享状态时使受影响实例停止接受写入，不假装可继续。`panic=abort`、内存耗尽等进程终止不能保证转成返回值。外部语言异常也不能穿过本库边界。参见 [Rust FFI 的 unwinding 约束](https://doc.rust-lang.org/nomicon/ffi.html#ffi-and-unwinding)。

FFI 所需的 `unsafe` 只存在于经过审查的薄边界，核心业务保持安全 Rust；不能直接沿用 MOMO 全 workspace `unsafe_code = deny` 后再声称已经完成原生指针适配。

## 6. 实现阶段的验收

纯 C 程序应能只包含头文件并链接动态库，完成：打开实例 → 配置服务商、模型与模型参数组 → 为三用途选择组 → 导入 MOC → 创建场景并设置注入范围 → 注册数据文件并提交响应 → 拉取增量与终态 → 修改消息并读取重新提炼结果 → 释放缓冲和任务 → 关闭实例。Rust API 与 HTTP 提供同样的业务行为，C ABI 不替代或取消它们。

同时验证 C/C++ 头文件编译、目标 ABI 的结构大小和对齐、符号导出、版本不匹配、无效 JSON、空输入、超时、取消与完成竞态、忙碌关闭、跨线程提交，以及输出缓冲释放。实现后再用真实跨语言宿主验证；只有 Rust 单元测试通过不足以证明 C ABI 可用。
