#ifndef MOLITE_H
#define MOLITE_H
#include <stdint.h>
#include <stddef.h>
#if defined(_WIN32) && !defined(MOLITE_STATIC)
#define MOLITE_API __declspec(dllimport)
#else
#define MOLITE_API
#endif
#ifdef __cplusplus
extern "C" {
#endif
typedef uint64_t molite_engine;
typedef uint64_t molite_task;
typedef struct molite_buffer { uint8_t *data; size_t len; } molite_buffer;
enum { MOLITE_OK=0, MOLITE_EMPTY=1, MOLITE_INVALID=2, MOLITE_BUSY=3, MOLITE_FAILED=4 };
MOLITE_API uint32_t molite_abi_version(void);
#ifdef __ANDROID__
/* Call from a JVM-owned thread before molite_open; requires matching verifier AAR. */
MOLITE_API int32_t molite_android_init(void *jni_env, void *application_context);
#endif
MOLITE_API int32_t molite_open(const uint8_t *path, size_t len, molite_engine *out);
MOLITE_API int32_t molite_submit(molite_engine engine,const uint8_t *json,size_t len,molite_task *out);
MOLITE_API int32_t molite_task_next(molite_task task,uint32_t wait_ms,molite_buffer *out);
MOLITE_API int32_t molite_task_cancel(molite_task task);
MOLITE_API int32_t molite_task_release(molite_task task);
MOLITE_API int32_t molite_close(molite_engine engine);
MOLITE_API void molite_buffer_free(molite_buffer buffer);
#ifdef __cplusplus
}
#endif
#endif
