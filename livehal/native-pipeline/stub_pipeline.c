// Stub definitions of the `source_pipeline_*` C ABI, compiled by
// livehal/build.rs ONLY when the CMake build of the real C++ pipeline did
// not run (no C++ capture/encoder backend feature enabled, or a non-Linux
// target).  They exist so Rust-only feature combinations — e.g.
// `encoder-opus` without any C++ backend — still link: liveion's
// `NativeEncodedSource` video path references these symbols whenever the
// `native-source` feature is compiled, regardless of whether any C++
// backend is built.
//
// The real header (`include/source_pipeline_ffi.h`) cannot be included
// from C — its declarations use C++ `noexcept` — so this file is
// self-contained: the handle is redeclared as an opaque struct and the
// config/hooks structs are only referenced through opaque pointers.
//
// Every entry point fails closed: `create` reports the missing backend in
// the caller's errbuf and returns NULL, `start` returns false, the
// lifecycle/keyframe functions are no-ops, `is_running`/`set_bitrate`
// return false.

#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>

typedef struct SourcePipelineHandle SourcePipelineHandle;

static const char kStubMsg[] =
    "no native capture/encoder backend compiled in (stub)";

static void write_errbuf(char *errbuf, size_t errbuf_len) {
    if (errbuf == NULL || errbuf_len == 0) {
        return;
    }
    size_t n = strlen(kStubMsg);
    if (n >= errbuf_len) {
        n = errbuf_len - 1;
    }
    memcpy(errbuf, kStubMsg, n);
    errbuf[n] = '\0';
}

SourcePipelineHandle *source_pipeline_create(const void *cfg,
                                             const void *hooks,
                                             char *errbuf,
                                             size_t errbuf_len) {
    (void)cfg;
    (void)hooks;
    write_errbuf(errbuf, errbuf_len);
    return NULL;
}

bool source_pipeline_start(SourcePipelineHandle *h,
                           char *errbuf,
                           size_t errbuf_len) {
    (void)h;
    write_errbuf(errbuf, errbuf_len);
    return false;
}

void source_pipeline_stop(SourcePipelineHandle *h) { (void)h; }

bool source_pipeline_is_running(SourcePipelineHandle *h) {
    (void)h;
    return false;
}

void source_pipeline_request_keyframe(SourcePipelineHandle *h) { (void)h; }

bool source_pipeline_set_bitrate(SourcePipelineHandle *h, unsigned int bps) {
    (void)h;
    (void)bps;
    return false;
}

void source_pipeline_free(SourcePipelineHandle *h) { (void)h; }
