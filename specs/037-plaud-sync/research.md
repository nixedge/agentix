# Research — 2026-11-17

## Owner-observed GATT result and approved advertisement diagnostics

Owner's v1 report collected2026-11-17T21:06:29Z on Android API37 confirms services1800/1801/1910/180F with command2BB1 properties0x0c and notify2BB0 properties0x10; Device Information180A is absent. All standard firmware/model reads therefore correctly report unavailable. This establishes GATT discovery, not firmware or sync compatibility. Owner explicitly approved advertisement diagnostics after this result.

The indexed OpenPlaudit code search's semantic embedding backend was unavailable; exact BM25 and indexed-file retrieval worked, but contained no firmware decoder. Upstream original research provides the missing field layout: [advertising parser](https://github.com/ORION2809/PLAUDE_EMULATOR_REV-ENG/blob/cd97ea6fec4f175088d35f07f0447c1535a1de4b/emulator/plaudsim/advertising.py), `parse_manufacturer_data`, derived from the public SDK's scan-parser bytecode. It describes project-width2, project u16LE, version-field length, ASCII type and little-endian numeric code of length-minus-one; serial begins at the next length byte. Firmware display uses type plus decimal code padded to at least4 digits. For known Plaud short-serial branches, protocol is a distinct u16LE at18 (888/881) or15 (880/882). Long/overlapping/truncated layouts cannot establish protocol. The companion [firmware research](https://github.com/ORION2809/PLAUDE_EMULATOR_REV-ENG/blob/cd97ea6fec4f175088d35f07f0447c1535a1de4b/docs/architecture/firmware.md) identifies manufacturer IDs89/70 (0x0059/0x0046).

New diagnostics support only known project codes, numeric widths1–4, valid type and complete serial bounds. Prefix hex ends before the serial-length byte; all serial/tail data, MAC and advertised name are omitted from copied reports. Manufacturer ID inventory, capture time and RSSI remain useful if decoding is unavailable. Synthetic test inputs follow this independent SDK-derived layout, but there is no owner's advertisement capture yet. The other [reverse-engineering reference](https://github.com/naeemarsalan/opendict/blob/main/PLAUD_BLE_Protocol_Specification.md#manufacturer-data-layout) mentions manufacturer firmware fields but omits the exact numeric-width detail, so it is not used to guess packet boundaries.

## Remote Bluetooth firmware inspection

Read actual standard Device Information values without a vendor handshake: service0x180A; firmware0x2A26, hardware0x2A27, software0x2A28, model0x2A24 and manufacturer0x2A29. A recorder may omit or restrict these characteristics; neither its advertisement name nor protocol10 establishes firmware. Preserve discovered UUID/property inventory even if a read times out, so a later vendor-specific implementation can use observed evidence. No firmware update/reset/read-unknown-vendor-command is part of inspection.

Primary references: [Bluetooth SIG assigned numbers](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Assigned_Numbers/out/en/index-en.html), [Device Information Service](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/DIS_v1.2/out/en/index-en.html), [Android GATT read callbacks](https://developer.android.com/reference/android/bluetooth/BluetoothGattCallback). API33+ uses callback value bytes; older Android uses the deprecated characteristic value callback. Reads stay serialized and abort on a missing callback rather than issuing an overlapping request.

Sources indexed through agentic-nix and checked against upstream:

- `Kurikara-dev/plaud-note-nb100-re` revision `9d840b3d5af9`: docs02–07. https://github.com/Kurikara-dev/plaud-note-nb100-re
- `mar-maradnus/openplaudit` revision `860eb1b75174`: `src/plaude/ble/{protocol,client,transfer}.py`, `src/plaude/audio/decoder.py`, `llms.txt/ble-protocol.md`. https://github.com/mar-maradnus/openplaudit/tree/860eb1b75174
- Android GATT operations/callbacks: https://developer.android.com/reference/android/bluetooth/BluetoothGatt
- Ogg Opus specification: https://www.rfc-editor.org/rfc/rfc7845

Protocol facts: service1910, notify2BB0/write2BB1; plaintext command header type1/u16LE command. Handshake request type2/zero/zero +32ASCII token. Status at payload0; protocol u16 at1. Limit initial implementation to protocol10. Binding mismatch status1 must never lead to automatic depair. Owner confirms official-app binding.

Session-list payload: 4opaque bytes, u32LE count, count entries(u32 session/u32 size/u16 scene). Reject incomplete lists rather than the reference client's truncated-prefix behavior. CMD26 request fields use the documented zero session/timestamp/retry payload. Transfer CMD28 fields are u32, not u64. Data type2 contains89-byte voice records: session u32, position u32, length u8,80-byte audio slot. Reference implementation concatenates records but does not validate offsets; one document calls offset PCM while NB100 notes describe file offsets. Initial strict profile uses compressed-byte offsets; this must be verified physically and an incompatible offset is surfaced rather than guessed. Head CMD28 payload session/status; tail CMD29 session/CRC16. Device may sendFFFF to indicate no checksum. Reference checksum covers wire records and sends CMD116 type0/u16CRC, awaits117. Never send30 to stop:30 deletes device recordings. Closing GATT safely aborts without a remote mutation.

No public source validates owner's device. Unknown packet shapes fail closed. The app will expose the observed protocol and numeric status without logging packet/token contents. Explicit token configuration supports the existing binding; obtaining that token or moving the binding remains a physical setup decision.

Ogg packaging avoids a native codec dependency: mono OpusHead, input rate16000, preskip0 (unknown encoder delay), OpusTags,960 ticks per20ms packet on the mandated48kHz granule clock; Ogg's non-reflected CRC32 per page. Container framing alone cannot establish real recording quality.
