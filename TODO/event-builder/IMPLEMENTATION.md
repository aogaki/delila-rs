# Event Bridge 実装詳細計画

**Date:** 2026-01-27
**Status:** ❌ OBSOLETE — Event Bridge は SPEC v0.5 (2026-05-19) で retire 済み(Online EB は Merger PUB 直接購読)。さらに v0.8 (2026-08-04) で EB 自体が C++ (root_sink 系譜) へ移行決定([TODO 66](66_cpp_event_builder.md))。root_sink が MessagePack ワイヤを直接読むため、固定バイナリへの変換ブリッジは恒久的に不要。本文書は歴史的記録として保存
**仕様書:** `docs/event_bridge_wire_format.md`

---

## 1. 概要

Merger の MessagePack PUB 出力を、C++ Event Builder が読める
固定バイナリフォーマットに変換するステートレスなブリッジバイナリを実装する。

**設計原則:** KISS — 状態管理なし、コマンドタスクなし、Operator 制御なし。
起動して Ctrl+C で停止するだけの純粋なフォワーダ。

---

## 2. アーキテクチャ

```
┌──────────────────────────────────────────────────┐
│                event_bridge                       │
│                                                   │
│  ┌─────────────┐         ┌─────────────────────┐ │
│  │  ZMQ SUB    │  async  │  ZMQ PUB            │ │
│  │  (Merger)   │────────▶│  (固定バイナリ)      │ │
│  │             │         │                     │ │
│  │ MessagePack │  変換   │ 14 bytes/hit packed │ │
│  └─────────────┘         └─────────────────────┘ │
│                                                   │
│  Ctrl+C → グレースフルシャットダウン              │
└──────────────────────────────────────────────────┘
```

**不要な機能 (KISS):**
- ~~コマンドタスク (REP ソケット)~~ — Operator 制御不要
- ~~状態管理 (ComponentState)~~ — 常時転送、状態遷移なし
- ~~統計カウンタ~~ — 初期版では不要 (将来追加可能)

---

## 3. ファイル構成

### 3.1 新規ファイル

| ファイル | 行数目安 | 内容 |
|---------|---------|------|
| `src/bin/event_bridge.rs` | ~100行 | ブリッジバイナリ本体 |

### 3.2 変更ファイル

| ファイル | 変更内容 |
|---------|---------|
| `Cargo.toml` | `[[bin]]` セクション追加 |

### 3.3 変更不要

| ファイル | 理由 |
|---------|------|
| `src/lib.rs` | ライブラリモジュール追加不要 (全ロジックが bin 内で完結) |
| `src/common/mod.rs` | 既存の Message/EventData をそのまま使用 |
| `src/merger/` | Merger のゼロコピー設計に影響なし |
| `config.toml` | CLI 引数のみで動作 (設定ファイル対応は将来) |

---

## 4. コード設計

### 4.1 CLI 引数

```rust
#[derive(Parser, Debug)]
#[command(name = "event_bridge", about = "MessagePack → Binary bridge for C++ Event Builder")]
struct Args {
    /// Merger PUB アドレス (SUB 接続先)
    #[arg(short = 's', long, default_value = "tcp://localhost:5556")]
    sub_address: String,

    /// Event Builder 向け PUB アドレス (BIND)
    #[arg(short = 'p', long, default_value = "tcp://*:5600")]
    pub_address: String,
}
```

### 4.2 main 関数

```
1. tracing 初期化
2. CLI 引数パース
3. ZMQ コンテキスト作成
4. SUB ソケット作成 → Merger に connect
5. PUB ソケット作成 → bind
6. Ctrl+C シグナルハンドラ設定
7. メインループ:
   a. SUB から受信
   b. Message::from_msgpack() でデシリアライズ
   c. match msg_type:
      - Data → encode_data(&batch.events) → PUB 送信
      - EndOfStream → encode_control(0x02) → PUB 送信
      - Heartbeat → encode_control(0x03) → PUB 送信
   d. Ctrl+C で break
8. ソケットクローズ、終了
```

### 4.3 エンコード関数

`docs/event_bridge_wire_format.md` セクション5.1 に定義済み。
bin ファイル内にインライン実装する (モジュール分離不要)。

```rust
fn encode_data(events: &[EventData]) -> Vec<u8>
fn encode_control(msg_type: u8) -> Vec<u8>
```

### 4.4 エラーハンドリング

| エラー | 対処 |
|--------|------|
| ZMQ 接続失敗 | エラーログ出力して終了 |
| デシリアライズ失敗 | warn ログ、メッセージをスキップ |
| PUB 送信失敗 | warn ログ、次のメッセージへ |

---

## 5. Cargo.toml 変更

```toml
[[bin]]
name = "event_bridge"
path = "src/bin/event_bridge.rs"
```

既存の依存で全て足りる:
- `tmq` — ZeroMQ async
- `futures` — StreamExt
- `rmp-serde` — MessagePack デシリアライズ (common::Message 経由)
- `clap` — CLI 引数
- `tracing` — ログ
- `tokio` — async ランタイム

---

## 6. 参照する既存パターン

| パターン | 参照先 | 使用箇所 |
|---------|--------|---------|
| SUB ソケット作成 | `src/monitor/mod.rs:658-668` | SUB 接続 |
| MessagePack デシリアライズ | `src/monitor/mod.rs:800-833` | Message::from_msgpack() |
| ZMQ PUB 送信 | `src/merger/mod.rs:484-506` | PUB 送信 |
| Ctrl+C ハンドラ | `src/bin/data_sink.rs` | シャットダウン |
| CLI 引数 | `src/common/cli.rs` | clap derive |

---

## 7. テスト計画

### 7.1 ビルド検証

```bash
cargo build --release
cargo clippy -- -D warnings
cargo test
```

### 7.2 手動統合テスト

```bash
# ターミナル 1: エミュレータ + Merger 起動
./scripts/start_daq.sh

# ターミナル 2: Event Bridge 起動
./target/release/event_bridge \
  --sub-address tcp://localhost:5556 \
  --pub-address tcp://*:5600

# ターミナル 3: Python で PUB 出力を検証
python3 verify_bridge.py
```

### 7.3 検証スクリプト (Python)

```python
import zmq
import struct

ctx = zmq.Context()
sub = ctx.socket(zmq.SUB)
sub.connect("tcp://localhost:5600")
sub.setsockopt(zmq.SUBSCRIBE, b"")

while True:
    msg = sub.recv()
    msg_type = msg[0]
    n_hits = struct.unpack_from('<I', msg, 1)[0]
    print(f"type=0x{msg_type:02x}, n_hits={n_hits}, size={len(msg)}")
    assert len(msg) == 5 + 14 * n_hits, "Size mismatch!"

    for i in range(min(n_hits, 3)):  # 最初の3ヒットを表示
        offset = 5 + 14 * i
        mod, ch, e, es = struct.unpack_from('<BBHH', msg, offset)
        ts = struct.unpack_from('<d', msg, offset + 6)[0]
        print(f"  hit[{i}]: mod={mod} ch={ch} energy={e} "
              f"energy_short={es} ts={ts:.3f} ns")
```

---

## 8. 将来の拡張 (今回は実装しない)

| 拡張 | 説明 | トリガー条件 |
|------|------|-------------|
| config.toml 対応 | `[network.event_bridge]` セクション | DAQ 起動スクリプト統合時 |
| Operator 制御 | コマンドタスク追加 | パイプライン一括制御が必要な場合 |
| 統計カウンタ | 受信/送信バッチ数のログ | パフォーマンスデバッグ時 |
| flags 転送 | Hit 構造体を 22 bytes に拡張 | C++ 側で flags が必要になった場合 |
| waveform 転送 | 別メッセージタイプ (0x04) | オンラインモニタで波形が必要な場合 |

---

## 変更履歴

| 日付 | 変更内容 |
|------|----------|
| 2026-01-27 | 初版作成 |
