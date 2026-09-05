#ifndef GGFM_SERVER_H
#define GGFM_SERVER_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

uint32_t ggfm_server_abi_version(void);
const char *ggfm_server_policy_sha256(void);
int32_t ggfm_server_start(const char *data_dir_utf8, void *android_asset_manager,
                          int64_t device_unix_seconds,
                          int32_t utc_offset_minutes,
                          const char *capability_utf8);
const char *ggfm_server_endpoint(void);
int32_t ggfm_server_begin_login(int64_t device_unix_seconds, int32_t utc_offset_minutes);
int64_t ggfm_server_active_usn(void);
size_t ggfm_server_drain_logs(char *buffer, size_t capacity);
int32_t ggfm_server_prepare_shutdown(void);
int32_t ggfm_server_stop(void);

#ifdef __cplusplus
}
#endif

#endif
