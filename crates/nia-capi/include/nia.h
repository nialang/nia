/* SPDX-License-Identifier: GPL-3.0-or-later */
#ifndef NIA_CAPI_H
#define NIA_CAPI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define NIA_CAPI_ABI_VERSION 1

typedef enum NiaStatus {
    NIA_STATUS_OK = 0,
    NIA_STATUS_DIAGNOSTICS = 1,
    NIA_STATUS_INVALID_INPUT = 2,
    NIA_STATUS_INTERNAL_ERROR = 3,
    NIA_STATUS_IO_ERROR = 4,
    NIA_STATUS_LINKER_ERROR = 5,
    NIA_STATUS_INVALID_ARTIFACT_REQUEST = 6,
} NiaStatus;

typedef enum NiaRuntime {
    NIA_RUNTIME_BARE = 0,
    NIA_RUNTIME_FREESTANDING = 1,
} NiaRuntime;

typedef enum NiaOptimizationLevel {
    NIA_OPTIMIZATION_O0 = 0,
    NIA_OPTIMIZATION_O1 = 1,
    NIA_OPTIMIZATION_O2 = 2,
    NIA_OPTIMIZATION_O3 = 3,
    NIA_OPTIMIZATION_OS = 4,
    NIA_OPTIMIZATION_OZ = 5,
} NiaOptimizationLevel;

typedef struct NiaString {
    uint8_t *ptr;
    size_t len;
} NiaString;

typedef struct NiaSession NiaSession;
typedef struct NiaCheckRequest NiaCheckRequest;
typedef struct NiaLinkOptions NiaLinkOptions;
typedef struct NiaResult NiaResult;

uint32_t nia_capi_abi_version(void);
NiaString nia_version(void);
NiaString nia_status_name(NiaStatus status);

NiaSession *nia_session_new(void);
void nia_session_free(NiaSession *session);

NiaCheckRequest *nia_check_request_new(const uint8_t *path_ptr, size_t path_len);
void nia_check_request_free(NiaCheckRequest *request);
NiaStatus nia_check_request_add_module(
    NiaCheckRequest *request,
    const uint8_t *name_ptr,
    size_t name_len,
    const uint8_t *path_ptr,
    size_t path_len);
NiaStatus nia_check_request_set_runtime(NiaCheckRequest *request, NiaRuntime runtime);
NiaStatus nia_check_request_set_optimization(
    NiaCheckRequest *request,
    NiaOptimizationLevel level);

NiaLinkOptions *nia_link_options_new(void);
void nia_link_options_free(NiaLinkOptions *options);
NiaStatus nia_link_options_add_arg(
    NiaLinkOptions *options,
    const uint8_t *arg_ptr,
    size_t arg_len);
NiaStatus nia_link_options_set_linker(
    NiaLinkOptions *options,
    const uint8_t *program_ptr,
    size_t program_len);
NiaStatus nia_link_options_set_dynamic_linker_auto(NiaLinkOptions *options);
NiaStatus nia_link_options_set_no_dynamic_linker(NiaLinkOptions *options);
NiaStatus nia_link_options_set_dynamic_linker_path(
    NiaLinkOptions *options,
    const uint8_t *path_ptr,
    size_t path_len);
NiaStatus nia_link_options_add_library_path(
    NiaLinkOptions *options,
    const uint8_t *path_ptr,
    size_t path_len);
NiaStatus nia_link_options_add_rpath(
    NiaLinkOptions *options,
    const uint8_t *path_ptr,
    size_t path_len);
NiaStatus nia_link_options_add_library(
    NiaLinkOptions *options,
    const uint8_t *name_ptr,
    size_t name_len);

NiaResult *nia_session_check(NiaSession *session, const NiaCheckRequest *request);
NiaResult *nia_session_emit_object_file(
    NiaSession *session,
    const NiaCheckRequest *request,
    const uint8_t *output_ptr,
    size_t output_len);
NiaResult *nia_session_emit_object_directory(
    NiaSession *session,
    const NiaCheckRequest *request,
    const uint8_t *output_ptr,
    size_t output_len);
NiaResult *nia_session_emit_executable(
    NiaSession *session,
    const NiaCheckRequest *request,
    const uint8_t *output_ptr,
    size_t output_len);
NiaResult *nia_session_emit_executable_with_options(
    NiaSession *session,
    const NiaCheckRequest *request,
    const uint8_t *output_ptr,
    size_t output_len,
    const NiaLinkOptions *options);
NiaResult *nia_check_file(const uint8_t *path_ptr, size_t path_len);
NiaStatus nia_result_status(const NiaResult *result);
NiaString nia_result_message(const NiaResult *result);
void nia_result_free(NiaResult *result);
void nia_string_free(NiaString value);

#ifdef __cplusplus
}
#endif

#endif
