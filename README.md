# bpsr-module-optimizer

Blue Protocol: Star Resonance（星痕）の所持モジュールから、条件に合う 4 枠の組み合わせを探索する Windows 向けデスクトップツールです。

所持モジュールはゲーム通信の読み取りから自動取得します。通信を取得できない環境向けに、`owned_modules.json` を読み込むフォールバックも用意しています。

> 個人利用向けの補助ツールです。ゲームデータの改変や通信内容の改ざんは行いません。利用は自己責任で、各サービスの利用規約を確認してください。

## 主な機能

- 所持モジュールから 4 枠の組み合わせを全探索
- 目標属性と除外属性の指定
- 属性ごとの最低 Lv 指定
- モジュールカテゴリによる絞り込み
- 上位 3 / 5 / 10 件の表示
- 結果ごとの Lv6 数、Lv5 数、属性レベル内訳、リンク効果（全属性値の合計）の表示
- モジュール情報のライブ取得と、条件指定後の自動再探索
- `owned_modules.json` からの読み込み

## 最適化基準

候補になる 4 枠の組み合わせを、次の優先順位で比較します。

1. 選択した目標属性が Lv6 に到達した数
2. 全属性の Lv6 数
3. 全属性の Lv5 数
4. 全属性レベルの合計
5. リンク効果（全属性値の合計）

属性レベルは、属性値の合計が `1 / 4 / 8 / 12 / 16 / 20` に到達するごとに Lv1 から Lv6 として扱います。

## 動作環境

- Windows 10 / 11 64-bit
- ライブ取得には管理者権限が必要

ライブ取得では WinDivert を使ってパケットを読み取ります。`WinDivert.dll` と `WinDivert64.sys` はインストーラに同梱され、アプリ本体と同じ場所に配置されます。

## インストール

1. [Releases](https://github.com/Rererr/bpsr-module-optimizer/releases) から NSIS インストーラ（`.exe`）をダウンロードします。
2. インストーラを実行してアプリをインストールします。
3. ライブ取得を使う場合は、アプリを管理者として実行します。
4. ゲーム内でマップ移動または再ログインを行うと、所持モジュールが読み込まれます。
5. 左側で条件を指定し、「最適化を実行」します。

## JSON ダンプの読み込み

ライブ取得を使わない場合は、`owned_modules.json` を読み込めます。

- 既定では、アプリの実行ファイルと同じディレクトリにある `owned_modules.json` を起動時に読み込みます。
- 環境変数 `BPSR_MODULE_DUMP` を指定すると、そのパスの JSON を読み込みます。
- アプリ内の再読み込み操作で、現在のダンプ内容を反映できます。

## 開発

前提:

- [Rust](https://rustup.rs/) stable
- [Node.js](https://nodejs.org/) 20 以上
- Windows でライブ取得を試す場合は管理者権限

```bash
npm install
npm run tauri dev
```

NSIS インストーラを作成する場合:

```bash
npm run tauri build
```

フロントエンドのみをビルドする場合:

```bash
npm run build
```

Rust 側の確認:

```bash
cd src-tauri
cargo check
```

## 構成

- `src/`: React フロントエンド
- `src-tauri/`: Tauri アプリ本体、最適化 API、ライブ取得連携
- `bpsr-core/`: パケット取得・解析などの共通ロジック

## 仕組み

- ゲームの `WorldEnterSnapshot` に含まれる所持アイテムとモジュール情報を読み取ります。
- `item_package` 内でモジュール属性（`mod_new_attr.mod_parts`）を持つアイテムを抽出します。
- `mod.mod_infos[key].init_link_nums` と突き合わせて、属性 ID と属性値を復元します。
- 属性名とモジュール種別名は、日本語表示用のローカライズデータを使って表示します。

## ライセンス

GPL-3.0-only です。詳細は [LICENSE](./LICENSE) を参照してください。

## 支援

開発の継続を支援したい場合は、[GitHub Sponsors](https://github.com/sponsors/Rererr) からサポートできます。

## クレジット

- [WinDivert](https://www.reqrypt.org/windivert.html): パケット取得に使用。WinDivert は GNU Lesser General Public License（LGPL）で提供されています。

ゲーム内名称や関連する権利は各権利者に帰属します。本ツールは非公式のファンメイドツールであり、ゲーム運営とは関係ありません。
