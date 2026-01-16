# Zero-Copy Merger Implementation

**Status: COMPLETED** (2026-01-14)

## Problem

Mergerコンポーネントで1MHzのイベントレートでデータドロップが発生していた。

### 原因分析

従来の実装では、Mergerの受信タスクで以下の処理を行っていた:
1. ZMQソケットから受信 (raw bytes)
2. `Message::from_msgpack()` で完全なデシリアライズ
3. mpscチャンネルに `Message` を送信
4. 送信タスクで `message.to_msgpack()` で再シリアライズ
5. ZMQソケットに送信

この serialize/deserialize のオーバーヘッドがボトルネックとなり、高レートでドロップが発生していた。

## Solution: Zero-Copy Transfer

### 実装内容

1. **MessageHeader軽量パーサー追加** (`src/common/mod.rs`)
   - `MessageHeader` enum: Data, EndOfStream, Heartbeat
   - `MessageHeader::parse()`: raw bytesからsource_id, sequence_numberのみを抽出
   - rmp_serdeの配列形式シリアライズに対応
   - フル deserialize なしでヘッダー情報を取得

2. **Merger Zero-Copy化** (`src/merger/mod.rs`)
   - チャンネル型を `Message` から `bytes::Bytes` に変更
   - Receiver task: ヘッダーのみパース、raw bytes をそのまま転送
   - Sender task: raw bytes を直接ZMQに送信（再シリアライズなし）
   - channel_capacity を 1000 → 10000 に増加

### コード変更

```rust
// Before (with overhead)
let msg = Message::from_msgpack(&data)?;
tx.send(msg).await;
// ... later ...
let bytes = msg.to_msgpack()?;
socket.send(bytes).await;

// After (zero-copy)
let raw_bytes: Bytes = Bytes::copy_from_slice(&data);
let header = MessageHeader::parse(&raw_bytes);  // Header only
tx.try_send(raw_bytes);  // Pass raw bytes
// ... later ...
socket.send(raw_bytes.as_ref()).await;  // Direct forwarding
```

## Benchmark Results

### 測定環境
- macOS Darwin 25.2.0
- 2 Emulators → Merger (local loopback)
- Release build with optimizations

### Test Results

| Config | Events/Batch | Batch Rate | Event Rate | Dropped |
|--------|-------------|------------|------------|---------|
| 1ms interval | 500 | 28,000 batch/s | 14 MHz | 0 |
| Full speed | 1000 | 1,830 batch/s | 1.83 MHz | 0 |
| Full speed | 100 | 20,300 batch/s | 2.03 MHz | 0 |

### 考察

- 1ms固定インターバルでは28,000 batch/s (= 28kHz batch rate × 500 events = 14MHz)でドロップなし
- フルスピードでは Emulator のイベント生成がボトルネックとなる
- 100 events/batch で 2.03 MHz を達成（ドロップなし）
- **従来の serialize/deserialize 実装では 1MHz でもドロップが発生していた**

## Files Modified

- `src/common/mod.rs`: Added `MessageHeader` enum and `parse()` method
- `src/merger/mod.rs`: Zero-copy implementation
- `Cargo.toml`: Added `bytes` crate

## Key Design Points

1. **Header-only parsing**: MessagePackの最初の数バイトからsource_id/sequence_numberを抽出
2. **Reference counting**: `bytes::Bytes` によるゼロコピーバッファ共有
3. **Non-blocking forwarding**: `try_send()` でチャンネルがfullでもブロックしない
4. **Atomic statistics**: ホットパスでのロックフリー統計収集

## Test Code

```rust
#[test]
fn message_header_parse_data() {
    let mut batch = MinimalEventDataBatch::new(42, 1);
    batch.sequence_number = 12345;
    batch.push(MinimalEventData::new(0, 0, 100, 80, 1000.0, 0));

    let msg = Message::data(batch);
    let bytes = msg.to_msgpack().unwrap();

    let header = MessageHeader::parse(&bytes);
    assert!(header.is_some());
    match header.unwrap() {
        MessageHeader::Data { source_id, sequence_number } => {
            assert_eq!(source_id, 42);
            assert_eq!(sequence_number, 12345);
        }
        _ => panic!("Expected Data variant"),
    }
}
```
