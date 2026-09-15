# Aldivine Protocol (PROTOCOL.md)

## Packet Header (32 bytes, little-endian)

| Offset | Field | Type | Notes |
|--------|-------|------|-------|
| 0 | protocol_version | u16 | must equal negotiated version (1) |
| 2 | session_id | u64 | per-connection session |
| 10 | channel | u8 | see Channel enum (0..=8) |
| 11 | sequence | u32 | per-channel sequence for dedup/ordering |
| 15 | tick | u32 | server/game tick at send |
| 19 | timestamp_ms | u64 | wall-clock ms |
| 27 | flags | u8 | bit0 RELIABLE, bit1 ORDERED |
| 28 | payload_len | u32 | bytes following header |

Followed by `payload_len` opaque bytes. Total frame = 32 + payload_len.
Maximum payload = 16 MiB (`MAX_PAYLOAD`). Frames exceeding this are rejected pre-gameplay.

## Logical Channels

0 Control, 1 Auth, 2 EventReliable, 3 EventUnreliable, 4 EntityState,
5 ResourceTransfer, 6 Admin, 7 Heartbeat, 8 VoiceMetadata.

Unreliable channels (EventUnreliable, EntityState, VoiceMetadata) are sent over
UDP datagrams or QUIC datagrams. Reliable channels use QUIC streams.

## Flags

- FLAG_RELIABLE (1): delivery must be acknowledged / retried.
- FLAG_ORDERED (2): preserve send order relative to same channel+session.

## Event Envelope (application layer)

Length-prefixed JSON: `u32 LE length` + `serde_json` of:

```json
{ "source_resource": "core", "event": "player_joined", "data": { ... }, "seq": 0 }
```

## Validation Rules (AstraNet Event Firewall — to be enforced at transport)

- reject unknown protocol_version;
- reject declared payload_len != actual bytes;
- reject payload > MAX_PAYLOAD;
- reject unknown channel;
- per-event schema + max size + rate limit enforced by resource capabilities.
- never trust client-reported IP; use server-observed remote address.
