# bpsr-module-optimizer

Star Resonance の所持モジュールから、選択属性の戦闘力スコアが最大になる 4/5 枠の組み合わせを探索する Tauri v2 アプリ。モジュール情報はゲーム通信から自動読取。Windows 専用・外部サーバなし。

## 構成

- `src/` — React + TypeScript + Vite + Tailwind(フロントエンド)
- `src-tauri/` — Rust バックエンド(探索ロジック、prost で protobuf 解析)
- `bpsr-core/` — パケット観測・解析クレート。`src-tauri/Cargo.toml` の path 依存 `../bpsr-core` が指す先で、**このリポジトリ内にベンダリングされたコピー**(git 管理下)。上流は bpsr-checker の `core/`。通信まわりの変更はまず上流を確認し、取り込みはこのコピーを更新する
- `src-tauri/tauri.conf.json` — NSIS バンドラ、WinDivert をリソース同梱

## コマンド

- 開発: `npm run tauri dev`(vite は port 1420)
- GPU探索版で開発/配布: `npm run tauri dev -- --features gpu` / `npm run tauri build -- --features gpu`(別インストーラとして配布。結果は CPU 版と完全一致)
- 型チェック+ビルド: `npm run build`(tsc && vite build)
- 配布ビルド: `npm run tauri build`

## 注意

- WinDivert 依存のため実機検証は管理者権限が必要
- **アプリが異常終了すると WinDivert ドライバがロードされたまま残り、`target/debug/WinDivert64.sys` のロックで以後のビルドが `os error 32` で失敗する**。`sc query WinDivert` が RUNNING なら管理者権限で `sc stop WinDivert`
- 検証用ダンプ: `testdata/owned_modules_230.json`(gitignore・非公開。実所持166件＋合成64件)。起動時に読ませるには `BPSR_MODULE_DUMP` に絶対パスを指定する。再生成は `node scripts/synthesize_dump.js <入力dump> <出力dump> <件数> [seed]`(シード固定で再現可能)
- 検証補助: `../module_diag.py`(`../extracted_game_data/owned_modules.json` の属性レベル集計)
- 複数クライアント同時起動時はキャラ別（`char_id`）にモジュールを保持・保存する。exe と同じディレクトリに生成される: `owned_modules_<char_id>.json`(キャラ別ダンプ)、`characters.json`(検知済みキャラ索引)。レガシー単一ファイル `owned_modules.json` は char_id=0 の「未特定」擬似キャラ専用として今も使う(削除・移行はしない)
- `BPSR_MODULE_DUMP` 指定時は指定ダンプを char_id=0 の擬似キャラとして読み込み、設定中はそれが常に有効キャラになる(実キャラをライブ検知していても優先。検証用途の明示的意図を尊重するため)。有効キャラの解決優先順位は `src-tauri/src/state.rs` の `Inner::effective_char` 参照
- 経緯・決定はメモリ `~/.claude/projects/C--Users-PCuser-develop-bpsr-module-optimizer/memory/` を参照
