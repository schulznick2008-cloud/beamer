/* beamer-transport C-ABI (SCHNITTSTELLEN.md 5). Erzeugt von cbindgen, nicht von Hand aendern. */

#ifndef BEAMER_TRANSPORT_H
#define BEAMER_TRANSPORT_H

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

typedef enum {
  BT_ROLE_SENDER = 1,
  BT_ROLE_RECEIVER = 2,
} BtRole;

typedef struct BtSession BtSession;

typedef struct {
  const uint8_t *data;
  size_t len;
  uint64_t pts_us;
  uint8_t fflags;
  uint32_t frame_id;
} BtFrame;

typedef void (*BtOnFrame)(void*, uint32_t, uint8_t, const BtFrame*);

typedef void (*BtOnControl)(void*, uint32_t, uint8_t);

typedef void (*BtOnStats)(void*, uint32_t, const char*);

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

BtSession *bt_open(uint16_t local_port, BtOnFrame on_frame, BtOnControl on_control, BtOnStats on_stats, void *ctx);

uint16_t bt_local_port(const BtSession *s);

int bt_set_peer(BtSession *s, const char *host, uint16_t port);

int bt_add_stream(BtSession *s, uint32_t stream_id, BtRole role, const uint8_t *s2r_key, const uint8_t *s2r_prefix, const uint8_t *r2s_key, const uint8_t *r2s_prefix, uint32_t max_kbps);

int bt_remove_stream(BtSession *s, uint32_t stream_id);

int bt_send_frame(BtSession *s, uint32_t stream_id, uint8_t kind, const uint8_t *data, size_t len, uint64_t pts_us, uint8_t fflags);

int bt_request_keyframe(BtSession *s, uint32_t stream_id);

uint32_t bt_target_kbps(const BtSession *s, uint32_t stream_id);

void bt_close(BtSession *s);

const char *bt_last_error(void);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* BEAMER_TRANSPORT_H */
