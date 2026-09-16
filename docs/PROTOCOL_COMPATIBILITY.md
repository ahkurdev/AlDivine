# AstraNet Protocol Compatibility

This document defines the wire compatibility, frame encoding, and channel layout of the AstraNet protocol in Project Aldivine.

## 1. Protocol Architecture

AstraNet operates on top of UDP with a multi-channel framing layer:
- **Channels**:
  - `CONTROL` (0): Handshake, MTU negotiation, channel synchronization
  - `AUTH` (1): Identity exchange, challenge/response, token validation
  - `EVENTS_RELIABLE` (2): Script net events with sequence numbering and ACK sliding window
  - `EVENTS_UNRELIABLE` (3): High-frequency ephemeral script events
  - `ENTITY_STATE` (4): ECS position, velocity, and rotation replication
  - `STATE_SYNC` (5): State bag synchronization across routing buckets
  - `RESOURCE_TRANSFER` (6): Resource manifest and metadata transfer
  - `ADMIN` (7): Aegis remote console and node control
  - `VOICE_METADATA` (8): Channel membership, speaker status, proximity tiers
  - `HEARTBEAT` (9): Round-trip time (RTT) calculation and connection keepalive

## 2. Packet Framing

Each AstraNet datagram begins with a fixed 32-byte binary header (`PacketHeader`):
- `protocol_version` (u16): Monotonically increasing wire version
- `session_id` (u32): Unique ephemeral session identifier
- `channel` (u8): Target AstraNet channel enum
- `flags` (u8): Bitflags (0x01 Reliable, 0x02 Ordered)
- `sequence` (u32): Packet sequence number
- `tick` (u64): Server or client virtual tick
- `timestamp_ms` (u64): Timestamp for jitter and lag compensation
- `payload_len` (u32): Payload length in bytes (max 16 MiB)

## 3. Compatibility Serialization

CitizenFX message serialization is handled via `ald-serialization-compat`:
- Support for Citizen typed MessagePack extensions:
  - Vector2 (type tag 20, 8 bytes)
  - Vector3 (type tag 21, 12 bytes)
  - Vector4 / Quat (type tag 22, 16 bytes)
- JSON fallback parser for legacy resources.
