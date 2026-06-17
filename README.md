# bpsr-module-optimizer

Blue Protocol: Star Resonance（星痕）の **モジュール（パワーコア）最適化ツール**。
所持しているモジュールから、指定した属性を優先しつつ **Lv6 が最も多くなる 4 枠の組み合わせ** を全探索で提案します。

- **ローカル動作**の Windows デスクトップアプリ（Tauri v2 + React）
- ゲーム通信を**パッシブ観測**して所持モジュールを自動取得（WinDivert / 要管理者権限）
- 取得できない環境向けに **JSON ダンプ読込**のフォールバックも内蔵

> ⚠️ 個人利用の補助ツールです。ゲームへの改変・干渉は行わず、通信の読み取りのみを行います。利用は自己責任で、各サービスの規約をご確認ください。

## 主な機能

- 属性のマルチセレクト（**目標 / 除外** をトグル）
- **属性ごとに下限 Lv を個別指定**（例: 極・絶境守護は Lv6 必須、集中・会心は下限なし）
- カテゴリ絞り込み（攻撃 / 支援 / 防御）
- 結果は **リンク効果** とともに、`Lv6×N` / `Lv5×N` と全属性のレベル内訳を表示
- マップ移動のたびにライブ取得 → 自動で再探索

### 最適化の基準（優先度順）

各 4 枠の組み合わせを、次の辞書式優先度で比較して最良を選びます。

1. 選択属性が **Lv6** に到達した数
2. **Lv6** 属性の総数（理想は 4 つ）
3. **Lv5** 属性の総数
4. 全属性の **レベル合計**
5. **リンク効果**（全属性値の合計＝ゲーム画面右上の数値）

属性レベルの閾値は `1 / 4 / 8 / 12 / 16 / 20`（= Lv1〜Lv6）です。

## 動作環境

- Windows 10/11（x64）
- ライブ取得は **管理者権限が必須**（WinDivert によるパケット観測のため）

## インストール / 使い方

1. [Releases](https://github.com/Rererr/bpsr-module-optimizer/releases) から NSIS インストーラ（`.exe`）を入手してインストール
2. **管理者として実行**（ライブ取得のため）
3. ゲーム内で**マップ移動 or 再ログイン**すると所持モジュールが取り込まれます
4. 左で目標属性を選び、必要なら下限 Lv を設定して「最適化を実行」

WinDivert（`WinDivert.dll` / `WinDivert64.sys`）はインストーラに同梱され、exe と同じ場所に配置されます。

## 開発・ビルド

前提: [Rust](https://rustup.rs/)（stable）, [Node.js](https://nodejs.org/) 18+。

```bash
npm install

# 開発（ライブ取得を試すなら管理者ターミナルから）
npm run tauri dev

# NSIS インストーラを生成
npm run tauri build
```

`bpsr-core`（パケット観測・解析の共有ロジック）は本リポジトリ内 `bpsr-core/` に
**vendoring（内製コピー）** 済みのため、追加リポジトリの取得なしで単体ビルドできます。

## 仕組み（概要）

- ゲームの `WorldEnterSnapshot`（ワールド/シーン入場時の1回）に、所持アイテムと
  モジュール装備情報が含まれます。
- `item_package` 内でモジュール属性（`mod_new_attr.mod_parts`）を持つアイテムを抽出し、
  `mod.mod_infos[key].init_link_nums` と突き合わせて「属性ID ↔ 値」を復元します。
- 属性名・モジュール種別名はゲームのローカライズデータ由来の日本語正式名を使用しています。

## ライセンス

**GPL-3.0-only**（[LICENSE](./LICENSE)）。

本アプリはパケット観測・解析に自作の `bpsr-core`（[bpsr-checker](https://github.com/Rererr/bpsr-checker) 由来、GPL-3.0-only）をリンク・同梱しているため、派生物として GPL-3.0-only で配布します。

## クレジット / 謝辞

- `bpsr-core`: 拙作 [bpsr-checker](https://github.com/Rererr/bpsr-checker) から vendoring（GPL-3.0-only）
- [WinDivert](https://www.reqrypt.org/windivert.html): パケット観測ドライバ（LGPLv3 / GPLv3）。`WinDivert.dll` / `WinDivert64.sys` を同梱
- モジュールシステムの解析にあたり、同ゲーム向けの先行 DPS / モジュールツール群を**仕様理解の参考**にしました（コードは流用していません）

属性名等のゲーム内名称の著作権は原権利者に帰属します。本ツールは非公式のファンメイドであり、ゲーム運営とは関係ありません。
