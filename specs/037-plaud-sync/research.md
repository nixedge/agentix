# Research — 2026-11-17

Sources indexed through agentic-nix and checked against upstream:

- `Kurikara-dev/plaud-note-nb100-re` revision `9d840b3d5af9`: docs02–07. https://github.com/Kurikara-dev/plaud-note-nb100-re
- `mar-maradnus/openplaudit` revision `860eb1b75174`: `src/plaude/ble/{protocol,client,transfer}.py`, `src/plaude/audio/decoder.py`, `llms.txt/ble-protocol.md`. https://github.com/mar-maradnus/openplaudit/tree/860eb1b75174
- Android GATT operations/callbacks: https://developer.android.com/reference/android/bluetooth/BluetoothGatt
- Ogg Opus specification: https://www.rfc-editor.org/rfc/rfc7845

Protocol facts: service1910, notify2BB0/write2BB1; plaintext command header type1/u16LE command. Handshake request type2/zero/zero +32ASCII token. Status at payload0; protocol u16 at1. Limit initial implementation to protocol10. Binding mismatch status1 must never lead to automatic depair. Owner confirms official-app binding.

Session-list payload: 4opaque bytes, u32LE count, count entries(u32 session/u32 size/u16 scene). Reject incomplete lists rather than the reference client's truncated-prefix behavior. CMD26 request fields use the documented zero session/timestamp/retry payload. Transfer CMD28 fields are u32, not u64. Data type2 contains89-byte voice records: session u32, position u32, length u8,80-byte audio slot. Reference implementation concatenates records but does not validate offsets; one document calls offset PCM while NB100 notes describe file offsets. Initial strict profile uses compressed-byte offsets; this must be verified physically and an incompatible offset is surfaced rather than guessed. Head CMD28 payload session/status; tail CMD29 session/CRC16. Device may sendFFFF to indicate no checksum. Reference checksum covers wire records and sends CMD116 type0/u16CRC, awaits117. Never send30 to stop:30 deletes device recordings. Closing GATT safely aborts without a remote mutation.

No public source validates owner's device. Unknown packet shapes fail closed. The app will expose the observed protocol and numeric status without logging packet/token contents. Explicit token configuration supports the existing binding; obtaining that token or moving the binding remains a physical setup decision.

Ogg packaging avoids a native codec dependency: mono OpusHead, input rate16000, preskip0 (unknown encoder delay), OpusTags,960 ticks per20ms packet on the mandated48kHz granule clock; Ogg's non-reflected CRC32 per page. Container framing alone cannot establish real recording quality.
