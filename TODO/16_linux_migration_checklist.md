# Linux Migration Checklist

**Created:** 2026-01-23
**Purpose:** scp -r でLinuxマシンにコピー後の確認事項

---

## Pre-Copy Checklist (macOS側)

コピー前に確認:
- [ ] `git status` で未コミットの変更を確認
- [ ] 重要な `.gitignore` ファイルの内容を確認

```bash
# 現在の状態を確認
git status
cat .gitignore
ls -la config/  # 設定ファイル
ls -la scripts/ # スクリプト
```

---

## Post-Copy Verification (Linux側)

### 1. Basic Environment

```bash
# Rustツールチェーン
rustc --version
cargo --version

# 必要なら最新版にアップデート
rustup update
```

### 2. CAEN FELib Dependencies

```bash
# FELibライブラリの確認
ls -la /usr/lib/libCAEN_FELib*
ldconfig -p | grep CAEN

# 環境変数（必要なら.bashrcに追加）
export LD_LIBRARY_PATH=/usr/lib:$LD_LIBRARY_PATH

# ヘッダーファイル確認
ls /usr/include/CAEN_FELib.h
```

**インストールされていない場合:**
- CAEN公式サイトからFELib SDKをダウンロード
- `legacy/` ディレクトリにインストーラがあるかも確認

### 3. Cargo Build

```bash
cd /path/to/delila-rs

# クリーンビルド（target/は.gitignoreなので再ビルド必要）
cargo clean
cargo build

# テスト実行
cargo test --lib

# Clippy
cargo clippy --all-targets
```

**ビルドエラー時の確認:**
- [ ] `build.rs` のbindgenパス
- [ ] CAEN FELibヘッダー/ライブラリパス
- [ ] pkg-configの設定

### 4. Configuration Files

```bash
# config.tomlの確認（IPアドレス等を環境に合わせて変更）
cat config.toml

# デジタイザ設定ファイルの確認
ls -la config/digitizers/

# 必要なら環境変数を設定
export CAEN_DIGITIZER_URL="dig2://YOUR_DIGITIZER_IP"
```

**config.tomlで変更が必要な可能性:**
- [ ] デジタイザURL (`digitizer_url`)
- [ ] MongoDB URI (`mongodb_uri`)
- [ ] ネットワークアドレス（bind/subscribe）

### 5. MongoDB Connection

```bash
# MongoDBが動作しているか確認
systemctl status mongod
# または
mongo --eval "db.adminCommand('ping')"

# 接続テスト（Operatorが使用）
mongosh mongodb://localhost:27017/delila --eval "db.runs.count()"
```

### 6. Frontend (Angular)

```bash
cd web/operator-ui

# Node.js/npm確認
node --version
npm --version

# 依存関係インストール（node_modulesは.gitignore）
npm install

# ビルド
npm run build

# 開発サーバー（オプション）
npm start
```

### 7. Hardware Integration Tests

```bash
# デジタイザ接続テスト（実機が接続されている場合）
cargo test --test felib_integration_test -- --ignored --test-threads=1

# 特定のテストのみ
cargo test test_connect_disconnect -- --ignored
cargo test test_get_device_info -- --ignored
```

### 8. Full System Test

```bash
# DAQシステム起動テスト
./scripts/start_daq.sh

# Operator API確認
curl http://localhost:8080/api/status

# Swagger UI確認
# ブラウザで http://localhost:8080/swagger-ui/

# 停止
./scripts/stop_daq.sh
```

### 9. PSD1 Specific (次のステップ)

PSD1はUSB/Optical接続のため、追加で確認:

```bash
# CAENDigitizerライブラリ（PSD1用、FELibとは別）
ls /usr/lib/libCAENDigitizer*

# USBパーミッション
ls -la /dev/usb/

# udevルール（必要なら設定）
cat /etc/udev/rules.d/99-caen.rules
```

---

## Troubleshooting

### ビルドエラー: FELib not found

```bash
# pkg-configパスを確認
pkg-config --libs --cflags caen_felib

# なければ手動で設定
export CAEN_FELIB_LIB_DIR=/usr/lib
export CAEN_FELIB_INCLUDE_DIR=/usr/include
```

### ランタイムエラー: shared library not found

```bash
# ldconfigを更新
sudo ldconfig

# または実行時にパス指定
LD_LIBRARY_PATH=/usr/lib:$LD_LIBRARY_PATH cargo run --bin operator
```

### Permission denied on USB device

```bash
# CAENデバイスのudevルール追加
sudo tee /etc/udev/rules.d/99-caen.rules << 'EOF'
SUBSYSTEM=="usb", ATTR{idVendor}=="21e1", MODE="0666"
EOF
sudo udevadm control --reload-rules
sudo udevadm trigger
```

---

## Files to Verify After Copy

| File/Directory | Description | Action |
|----------------|-------------|--------|
| `config.toml` | メイン設定 | IPアドレス確認・変更 |
| `config/digitizers/*.json` | デジタイザ設定 | 確認 |
| `scripts/*.sh` | 起動スクリプト | 実行権限確認 (`chmod +x`) |
| `.env` (存在すれば) | 環境変数 | 確認・変更 |
| `target/` | ビルド成果物 | 再ビルド必要 |
| `web/operator-ui/node_modules/` | npm依存 | `npm install` 必要 |

---

## Quick Verification Commands

```bash
# 全体の確認スクリプト
cd /path/to/delila-rs

echo "=== Rust ==="
cargo --version && cargo build --release

echo "=== Tests ==="
cargo test --lib

echo "=== Frontend ==="
cd web/operator-ui && npm install && npm run build && cd ../..

echo "=== Scripts ==="
chmod +x scripts/*.sh
ls -la scripts/

echo "=== Config ==="
cat config.toml | head -30

echo "=== Ready! ==="
```

---

## Notes

- `scp -r` はシンボリックリンクを実体コピーするので注意
- 大きなファイル（`target/`, `node_modules/`）は除外してコピーを高速化できる:
  ```bash
  rsync -avz --exclude 'target' --exclude 'node_modules' \
    /path/to/delila-rs user@linux-host:/path/to/
  ```
- Git履歴を含めてコピーされるので、Linux側でも `git status` が使える
