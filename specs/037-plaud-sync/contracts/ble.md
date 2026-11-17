# BLE contract

Outgoing allowlist:1(handshake),26(list),28(download),116(checksum). No generic public command builder, deletion, unbind, reset, settings or firmware command. Commands use type1 +u16LE ID. Only protocol10 is supported.

Transport opens1910, requestsMTU517 and requiresMTU>=41, enables2BB0 via2902 and writes2BB1 with response. Operations and notifications have bounded queues and finite deadlines; unsolicited command9 may be ignored. No Android callback performs file I/O. Unexpected required responses, malformed frames, queue overflow and disconnect terminate the current session. New connections use new callback/channel identities.

Listing requires complete count/entry payload. Transfer accepts CMD28 head, type2 audio, CMD29 tail; data may arrive before head acknowledgement but is buffered in order. Resume uses validated compressed-byte offset. Checksum acknowledgement117 must arrive before publication. Stop closes GATT, never sends30.

Authentication failure1 means existing binding/token mismatch;2 means recorder busy. Private token entry validates32ASCII hex characters without changing case. No credential retrieval, cloud login or bind replacement occurs automatically.

Separately confirmed migration command: `01 05 00 00` (CMD5, clearStorage0). This constructor is isolated in `PlaudBinding`; ordinary `PlaudSync` never references it. The UI confirmation is required on every migration attempt. Probe protocol10 before sending, persist token before mutating, never send clearStorage1. A successful GATT write is not proof of completed binding migration; owner retries explicit sync after device restart.
