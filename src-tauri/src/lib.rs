//! bpsr-module-optimizer: 所持モジュールから、Lv6数 → 評価リンク → Lv5数（既定の `Link`
//! モード）または Lv6数 → Lv5数 → 評価リンク（`Lv5` モード）の優先度で最良の枠
//! （4または5）の組み合わせを求める Tauri アプリ。ランキング順序モードは
//! `optimizer::RankMode` 参照。
//!
//! 複数クライアント（複数キャラ）同時起動に対応するため、モジュールはキャラ別
//! （`owned_modules_<char_id>.json`）に保存し、`characters.json` へキャラ索引を
//! 持つ（有効キャラの解決優先順位は `state` モジュール参照）。

mod attrs;
mod capture;
mod elevation;
mod optimizer;
#[cfg(feature = "gpu")]
mod optimizer_gpu;
mod state;

use attrs::AttrMeta;
use optimizer::{Module, OptimizeResult, RankMode};
use serde::{Deserialize, Serialize};
use state::{CharacterEntry, SharedState};
use std::path::{Path, PathBuf};

/// キャプチャ／取得状態のスナップショット。
#[derive(Serialize)]
struct StatusDto {
    /// "init" | "running" | "failed"
    capture_state: String,
    /// 有効キャラ（選択中、無ければ自動解決）のモジュール件数。
    module_count: usize,
    /// 有効キャラの最終更新時刻。
    last_update_ms: Option<u64>,
    /// 有効キャラの取得元（"capture" | "dump" | "none"）。
    source: String,
    /// ゲームパケットを最後に観測してからの経過 ms（None=未観測）。
    last_game_packet_ms_ago: Option<u64>,
    /// 検知済みキャラ一覧（検知順）。
    characters: Vec<CharacterDto>,
    /// 実際に有効になっているキャラの id（自動採用時もその id を返す）。
    /// char_id は i64 のため JS の number では精度が落ちうる。文字列で返す。
    selected_char_id: Option<String>,
}

/// フロント表示用のキャラ情報。char_id は i64（UID）の精度落ちを避けるため文字列で返す。
#[derive(Serialize, Clone)]
struct CharacterDto {
    char_id: String,
    name: String,
    last_update_ms: u64,
    module_count: usize,
}

/// characters.json（キャラ索引）の1エントリ。char_id は将来の外部ツール・手編集での
/// 扱いやすさのため文字列でシリアライズする（フロント DTO と表現を揃える）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct CharacterIndexEntry {
    char_id: String,
    name: String,
    last_update_ms: u64,
    module_count: usize,
}

/// owned_modules_<char_id>.json（bpsr-checker のダンプ）読込用の形。
#[derive(Deserialize)]
struct DumpPart {
    attr_id: i32,
    attr_name: String,
    value: i32,
}

#[derive(Deserialize)]
struct DumpModule {
    key: i64,
    uuid: i64,
    config_id: i32,
    name: String,
    quality: i32,
    parts: Vec<DumpPart>,
}

impl From<DumpModule> for Module {
    fn from(d: DumpModule) -> Self {
        Module {
            // 名称はゲーム公式の日本語名へ解決（未知はダンプの値にフォールバック）。
            name: attrs::module_name(d.config_id)
                .map(str::to_string)
                .unwrap_or(d.name),
            category: optimizer::category_of(d.config_id).to_string(),
            quality: d.quality,
            key: d.key,
            uuid: d.uuid,
            config_id: d.config_id,
            parts: d
                .parts
                .into_iter()
                .map(|p| optimizer::Part {
                    attr_name: attrs::attr_name(p.attr_id)
                        .map(str::to_string)
                        .unwrap_or(p.attr_name),
                    attr_id: p.attr_id,
                    value: p.value,
                })
                .collect(),
        }
    }
}

fn load_dump(path: &Path) -> Result<Vec<Module>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("読込失敗 {}: {e}", path.display()))?;
    let dump: Vec<DumpModule> =
        serde_json::from_str(&text).map_err(|e| format!("JSON 解析失敗: {e}"))?;
    Ok(dump.into_iter().map(Module::from).collect())
}

/// モジュールを JSON でパスへ書き出す（取得時の自動保存で使用）。
/// 出力は `DumpModule` が読み戻せる形（`category` 余剰フィールドは読込側で無視される）。
pub(crate) fn write_dump(path: &Path, modules: &[Module]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(modules).map_err(|e| format!("JSON 変換失敗: {e}"))?;
    std::fs::write(path, json).map_err(|e| format!("保存失敗 {}: {e}", path.display()))
}

/// キャラ索引 characters.json を書き出す。
pub(crate) fn write_characters_index(
    path: &Path,
    entries: &[CharacterIndexEntry],
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(entries).map_err(|e| format!("JSON 変換失敗: {e}"))?;
    std::fs::write(path, json).map_err(|e| format!("保存失敗 {}: {e}", path.display()))
}

/// キャラ索引 characters.json を読み込む。
fn load_characters_index(path: &Path) -> Result<Vec<CharacterIndexEntry>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("読込失敗 {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("JSON 解析失敗: {e}"))
}

/// キャラ索引を読み込む。ファイルが無いのは初回起動時の通常状態なので info、
/// それ以外（JSON破損等）は握り潰さず warn でログする。
fn load_characters_index_for_startup(path: &Path) -> Vec<CharacterIndexEntry> {
    if !path.exists() {
        log::info!(
            "[startup] キャラ索引ファイルなし（初回起動時は通常）: {}",
            path.display()
        );
        return Vec::new();
    }
    match load_characters_index(path) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[startup] キャラ索引の読込/解析に失敗: {e}");
            Vec::new()
        }
    }
}

/// 現在のキャラ一覧から characters.json 用のエントリ列を作る。
pub(crate) fn characters_index_from(chars: &[CharacterEntry]) -> Vec<CharacterIndexEntry> {
    chars
        .iter()
        .map(|c| CharacterIndexEntry {
            char_id: c.char_id.to_string(),
            name: c.name.clone(),
            last_update_ms: c.last_update_ms.unwrap_or(0),
            module_count: c.modules.len(),
        })
        .collect()
}

/// 現在時刻の unix ms。
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// ファイルの最終更新時刻（unix ms）。取得できなければ None（起動時読込で
/// 実際の鮮度が不明な場合に `now_ms()` で嘘の鮮度を作らないため）。
fn file_mtime_ms(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

/// キャラ別ダンプ・キャラ索引を置くディレクトリ（既定は exe と同じディレクトリ）。
pub(crate) fn modules_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|dir| dir.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// キャラ別ダンプのパス。
fn char_dump_path(dir: &Path, char_id: i64) -> PathBuf {
    dir.join(format!("owned_modules_{char_id}.json"))
}

/// レガシー・単一ダンプのパス（本機能追加前の唯一の保存先）。
fn legacy_dump_path(dir: &Path) -> PathBuf {
    dir.join("owned_modules.json")
}

/// キャラ索引 characters.json のパス。
pub(crate) fn characters_index_path(dir: &Path) -> PathBuf {
    dir.join("characters.json")
}

/// char_id に対応する保存先パス。char_id=0（未特定の擬似キャラ。レガシー単一ファイル/env
/// 指定ダンプ由来）は必ずレガシーパスへ保存する。実キャプチャの char_id はゲームの
/// UID なので 0 になることはなく、衝突しない。
pub(crate) fn dump_path_for(dir: &Path, char_id: i64) -> PathBuf {
    if char_id == 0 {
        legacy_dump_path(dir)
    } else {
        char_dump_path(dir, char_id)
    }
}

/// `BPSR_MODULE_DUMP` 環境変数で指定された検証用ダンプのパス（未設定なら None）。
fn env_dump_path() -> Option<PathBuf> {
    std::env::var_os("BPSR_MODULE_DUMP").map(PathBuf::from)
}

/// ディレクトリから `owned_modules_<char_id>.json` を走査する（レガシーの
/// `owned_modules.json` はこのパターンにマッチしないため混同しない）。
/// `read_dir` 順は OS 依存で非決定的なため、char_id 昇順にソートして返す。
fn scan_character_dump_files(dir: &Path) -> Vec<(i64, PathBuf)> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            log::warn!(
                "[startup] ダンプディレクトリの走査に失敗 {}: {e}",
                dir.display()
            );
            return Vec::new();
        }
    };
    let mut found = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(rest) = file_name.strip_prefix("owned_modules_") else {
            continue;
        };
        let Some(id_str) = rest.strip_suffix(".json") else {
            continue;
        };
        let Ok(char_id) = id_str.parse::<i64>() else {
            continue;
        };
        found.push((char_id, path));
    }
    found.sort_by_key(|(char_id, _)| *char_id);
    found
}

/// 起動時の事前読込。優先順位:
/// 1. `BPSR_MODULE_DUMP`（検証用途）→ char_id=0 の擬似キャラとして読込、`env_fixed` を確定。
///    指定パスが存在しない場合は設定ミスとして警告し、2以降へフォールスルーする
///    （`env_fixed` 自体は変数が設定されている限り確定したままにする）。
/// 2. キャラ別ファイル（`owned_modules_*.json`）が1つ以上あればそれらを読込
///    （名前・最終更新時刻は `characters.json` から復元、無ければファイルの mtime）。
/// 3. どちらも無ければレガシー単一ファイル `owned_modules.json` を char_id=0 の
///    擬似キャラとして読込。
///
/// `dir`/`env_path` を引数化しているのはユニットテストから注入するため
/// （`modules_dir()`/環境変数はプロセスグローバルでテスト間の競合を招くため使わない）。
fn load_at_startup_from(shared: &SharedState, dir: &Path, env_path: Option<PathBuf>) {
    if let Some(env_path) = &env_path {
        match shared.lock() {
            Ok(mut s) => s.env_fixed = true,
            Err(e) => log::warn!("[startup] state ロック取得に失敗（env_fixed 設定不可）: {e}"),
        }
        if !env_path.exists() {
            // 設定ミスの可能性が高いため info ではなく warn。ダンプ自体は読めないが、
            // キャラ別ファイル/レガシーダンプの読込は無関係なのでスキップせず続行する。
            log::warn!(
                "[startup] BPSR_MODULE_DUMP 指定パスが存在しません（設定を確認してください）: {}",
                env_path.display()
            );
        } else {
            match load_dump(env_path) {
                Ok(modules) => {
                    let count = modules.len();
                    let last_update_ms = file_mtime_ms(env_path);
                    match shared.lock() {
                        Ok(mut s) => {
                            let entry = s.upsert_char(0);
                            entry.modules = modules;
                            entry.source = "dump".to_string();
                            entry.last_update_ms = last_update_ms;
                        }
                        Err(e) => log::warn!("[startup] state ロック取得に失敗: {e}"),
                    }
                    log::info!(
                        "[startup] BPSR_MODULE_DUMP から {count} 件を事前読込: {}",
                        env_path.display()
                    );
                }
                Err(e) => log::warn!("[startup] BPSR_MODULE_DUMP 事前読込スキップ: {e}"),
            }
            return;
        }
    }

    let index = load_characters_index_for_startup(&characters_index_path(dir));
    let files = scan_character_dump_files(dir);

    if files.is_empty() {
        let legacy = legacy_dump_path(dir);
        if !legacy.exists() {
            log::info!("[startup] 事前読込できるダンプがありません");
            return;
        }
        match load_dump(&legacy) {
            Ok(modules) => {
                let count = modules.len();
                let last_update_ms = file_mtime_ms(&legacy);
                match shared.lock() {
                    Ok(mut s) => {
                        let entry = s.upsert_char(0);
                        entry.modules = modules;
                        entry.source = "dump".to_string();
                        entry.last_update_ms = last_update_ms;
                    }
                    Err(e) => log::warn!("[startup] state ロック取得に失敗: {e}"),
                }
                log::info!(
                    "[startup] レガシーダンプから {count} 件を事前読込: {}",
                    legacy.display()
                );
            }
            Err(e) => log::warn!("[startup] レガシーダンプ事前読込スキップ: {e}"),
        }
        return;
    }

    // characters.json に載っている順（＝過去の検知順）を優先し、索引に無い新規
    // ファイルは末尾に追加する。
    let mut ordered_ids: Vec<i64> = index
        .iter()
        .filter_map(|e| match e.char_id.parse::<i64>() {
            Ok(id) => Some(id),
            Err(err) => {
                log::warn!(
                    "[startup] キャラ索引の char_id が不正のためスキップ: {} ({err})",
                    e.char_id
                );
                None
            }
        })
        .collect();
    for (char_id, _) in &files {
        if !ordered_ids.contains(char_id) {
            ordered_ids.push(*char_id);
        }
    }

    let mut s = match shared.lock() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[startup] state ロック取得に失敗（キャラ別ダンプ事前読込を中断）: {e}");
            return;
        }
    };
    for char_id in ordered_ids {
        let Some((_, path)) = files.iter().find(|(id, _)| *id == char_id) else {
            continue;
        };
        match load_dump(path) {
            Ok(modules) => {
                let count = modules.len();
                let idx_entry = index.iter().find(|e| e.char_id == char_id.to_string());
                let entry = s.upsert_char(char_id);
                entry.modules = modules;
                entry.source = "dump".to_string();
                entry.name = idx_entry.map(|e| e.name.clone()).unwrap_or_default();
                // 索引の last_update_ms == 0 は「不明」を表す（CharacterIndexEntry 生成時の
                // unwrap_or(0)）。0 をそのまま採用すると UI が 1970 年起点の巨大な経過時間を
                // 表示しうるため、0/未索引ともファイルの mtime へフォールバックする。
                entry.last_update_ms = idx_entry
                    .map(|e| e.last_update_ms)
                    .filter(|v| *v > 0)
                    .or_else(|| file_mtime_ms(path));
                log::info!("[startup] キャラ{char_id} のダンプから {count} 件を事前読込");
            }
            Err(e) => log::warn!("[startup] キャラ{char_id} のダンプ読込スキップ: {e}"),
        }
    }
}

/// `load_at_startup_from` の実運用向けラッパ（実ディレクトリ・実環境変数を使う）。
fn load_at_startup(shared: &SharedState) {
    load_at_startup_from(shared, &modules_dir(), env_dump_path());
}

// ---- コマンド ----

#[tauri::command]
fn get_modules(state: tauri::State<SharedState>) -> Vec<Module> {
    let s = state.lock().expect("state poisoned");
    s.effective_char()
        .map(|c| c.modules.clone())
        .unwrap_or_default()
}

#[tauri::command]
fn get_attributes() -> Vec<AttrMeta> {
    attrs::all()
}

#[tauri::command]
fn capture_status(state: tauri::State<SharedState>) -> StatusDto {
    use bpsr_core::capture::status;
    let s = state.lock().expect("state poisoned");
    let capture_state = match status::state() {
        status::STATE_RUNNING => "running",
        status::STATE_FAILED => "failed",
        _ => "init",
    }
    .to_string();
    let last_game = status::ms_since(
        status::LAST_GAME_PACKET_UNIX_MS.load(std::sync::atomic::Ordering::Relaxed),
    );
    let characters: Vec<CharacterDto> = s
        .chars
        .iter()
        .map(|c| CharacterDto {
            char_id: c.char_id.to_string(),
            name: c.name.clone(),
            last_update_ms: c.last_update_ms.unwrap_or(0),
            module_count: c.modules.len(),
        })
        .collect();
    let effective = s.effective_char();
    StatusDto {
        capture_state,
        module_count: effective.map(|c| c.modules.len()).unwrap_or(0),
        last_update_ms: effective.and_then(|c| c.last_update_ms),
        source: effective
            .map(|c| c.source.clone())
            .unwrap_or_else(|| "none".to_string()),
        last_game_packet_ms_ago: last_game,
        characters,
        selected_char_id: s.effective_char_id().map(|id| id.to_string()),
    }
}

/// 表示/探索対象のキャラを切り替える。切替後のモジュール件数を返す。
/// 未知の id は握り潰さずエラー文字列を返す。
#[tauri::command]
fn select_character(
    app: tauri::AppHandle,
    state: tauri::State<SharedState>,
    char_id: String,
) -> Result<usize, String> {
    use tauri::Emitter;
    let id: i64 = char_id
        .parse()
        .map_err(|_| format!("不正なキャラID: {char_id}"))?;
    let count = {
        let mut s = state.lock().expect("state poisoned");
        if !s.chars.iter().any(|c| c.char_id == id) {
            return Err(format!("未知のキャラID: {char_id}"));
        }
        s.selected_char_id = Some(id);
        s.effective_char().map(|c| c.modules.len()).unwrap_or(0)
    };
    // フロントが選択後すぐモジュールを再取得できるよう、既存の再探索経路を再利用する。
    let _ = app.emit("modules-updated", count);
    Ok(count)
}

/// 候補の切り捨て（フロントの「候補の絞り込み」）で受け付ける上限。
/// フロント `src/App.tsx` の `CUTOFF_MAX_QUALITY` / `CUTOFF_MAX_VALUE` と一致させること。
///
/// 上限の根拠（512件×5パターン・slot5・top_k=10 の実測。担い手の少ない属性を目標にした
/// ケースを含む。詳細はプロジェクトメモリ optimizer-performance 参照）:
/// - 属性値合計: 8 までは全パターンで上位10件が全数探索と完全一致。10〜12 で2位以下が
///   崩れ始め、13 以上で1位が崩れた。候補一覧を見比べるアプリなので上位10件を基準に 8。
/// - 品質: quality>=3 は 15/15 の条件で無傷。quality>=4 は 4/15 で1位が崩れた。
const MAX_MIN_QUALITY: i32 = 4;
const MAX_MIN_MODULE_VALUE: i32 = 8;

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn optimize(
    state: tauri::State<'_, SharedState>,
    selected_ids: Vec<i32>,
    category: Option<String>,
    // ハード除外: いずれかを含むモジュールを候補から丸ごと除外。
    exclude_ids: Vec<i32>,
    // ソフト除外: モジュールは候補に残すが、該当属性はランキング集計から除外する。
    soft_exclude_ids: Vec<i32>,
    // 属性ごとの下限レベル要求 [(attr_id, min_level)]。level 0/未指定は制約なし。
    requirements: Vec<(i32, usize)>,
    top_k: usize,
    // 装備枠数。現状は 4 または 5 のみ対応。
    slot_count: usize,
    // ランキング順序モード。フロントは常に明示送信する。
    rank_mode: RankMode,
    // 候補の切り捨て（ユーザーの明示的な意思表示。既定は無効＝すべて候補に含める）。
    // 品質がこれ未満のモジュールを候補から外す。0/1 は実質無効（実データの下限は 2）。
    min_quality: i32,
    // 属性値の合計がこれ未満のモジュールを候補から外す。0 は無効。
    min_module_value: i32,
) -> Result<OptimizeResult, String> {
    // MutexGuard を await をまたいで保持しないよう、先にモジュールを複製する。
    let modules = state
        .lock()
        .expect("state poisoned")
        .effective_char()
        .map(|c| c.modules.clone())
        .unwrap_or_default();
    // 切り捨てはカテゴリ絞り込み・ハード除外と同じ「候補から外す」操作なので、探索
    // ロジックには手を入れずここで候補集合を絞る（candidate_count もフィルタ後になる）。
    // 上限はフロントの入力欄でも縛るが、コマンドは単体でも呼べるためここでも clamp する。
    let min_quality = min_quality.clamp(0, MAX_MIN_QUALITY);
    let min_module_value = min_module_value.clamp(0, MAX_MIN_MODULE_VALUE);
    let modules: Vec<Module> = modules
        .into_iter()
        .filter(|m| m.quality >= min_quality)
        .filter(|m| m.parts.iter().map(|p| p.value).sum::<i32>() >= min_module_value)
        .collect();
    let top_k = top_k.clamp(1, 100);
    // 想定外の値は握り潰さず 4〜5 に丸める。
    let slot_count = slot_count.clamp(4, 5);
    // 全探索は重いので blocking プールへ退避し UI スレッドを塞がない。
    // GPU版ビルド（feature "gpu"）では optimizer_gpu 経由（失敗時は内部でCPUへ委譲）。
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(feature = "gpu")]
        let f = optimizer_gpu::optimize;
        #[cfg(not(feature = "gpu"))]
        let f = optimizer::optimize;
        f(
            &modules,
            &selected_ids,
            category.as_deref(),
            &exclude_ids,
            &soft_exclude_ids,
            &requirements,
            top_k,
            slot_count,
            rank_mode,
        )
    })
    .await
    .map_err(|e| format!("最適化タスク失敗: {e}"))?
}

/// 指定パス（省略時は有効キャラの既定ダンプ）を読み込み、モジュールを差し替える。
/// パス明示時は従来どおり char_id=0（未特定の擬似キャラ）として読み込む（検証用途）。
/// 省略時、有効キャラが char_id=0 かつ `BPSR_MODULE_DUMP` 設定中はそのパスを優先する
/// （検証用途で env を指定している間は常にそのダンプを見る、という意図を尊重する）。
#[tauri::command]
fn reload_from_dump(
    state: tauri::State<SharedState>,
    path: Option<String>,
) -> Result<usize, String> {
    if let Some(p) = path {
        let modules = load_dump(&PathBuf::from(p))?;
        let count = modules.len();
        let mut s = state.lock().expect("state poisoned");
        let entry = s.upsert_char(0);
        entry.modules = modules;
        entry.last_update_ms = Some(now_ms());
        entry.source = "dump".to_string();
        return Ok(count);
    }

    let char_id = {
        let s = state.lock().expect("state poisoned");
        s.effective_char_id()
            .ok_or_else(|| "検知済みのキャラがありません".to_string())?
    };
    let p = if char_id == 0 {
        env_dump_path().unwrap_or_else(|| dump_path_for(&modules_dir(), 0))
    } else {
        dump_path_for(&modules_dir(), char_id)
    };
    let modules = load_dump(&p)?;
    let count = modules.len();
    let mut s = state.lock().expect("state poisoned");
    let entry = s.upsert_char(char_id);
    entry.modules = modules;
    entry.last_update_ms = Some(now_ms());
    entry.source = "dump".to_string();
    Ok(count)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    // ライブキャプチャ（WinDivert）は管理者権限を要求する。未昇格なら自己再起動で昇格する。
    // 昇格に失敗した場合（UAC 拒否含む）は本プロセスは終了する。
    elevation::ensure_elevated();

    let shared = state::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(shared.clone())
        .setup(move |app| {
            // 起動時: キャラ別ダンプ/レガシーダンプがあれば事前読込（キャプチャ前でも
            // UI に表示・キャラ切替ができる）。
            load_at_startup(&shared);
            // ライブキャプチャ開始（要管理者権限）。
            capture::spawn(app.handle().clone(), shared.clone());

            // GPU版ビルドはウィンドウタイトルで区別できるようにする（失敗しても起動は続行）。
            #[cfg(feature = "gpu")]
            {
                use tauri::Manager;
                if let Some(window) = app.get_webview_window("main") {
                    let base_title = window.title().unwrap_or_default();
                    if let Err(e) = window.set_title(&format!("{base_title} (GPU)")) {
                        log::warn!("[startup] ウィンドウタイトル設定失敗: {e}");
                    }
                    // GPUデバイス初期化・パイプラインコンパイルを先食いしておく（初回クエリの
                    // +0.2〜1.5s を隠す）。探索クエリを塞がないよう別スレッドで実行する
                    // （optimize コマンドと同じ optimizer_gpu::gpu_context の OnceLock を叩くだけ）。
                    // プリウォーム失敗時（=以後の探索クエリは毎回CPUへフォールバックする）は
                    // ウィンドウタイトルを (GPU→CPU) へ書き換え、ユーザーが計算方式の実態を
                    // 起動直後から把握できるようにする（per-query フォールバックは結果側の
                    // engine 表示が担うためタイトルは変えない）。
                    let app_handle = app.handle().clone();
                    std::thread::spawn(move || {
                        if optimizer_gpu::prewarm() {
                            return;
                        }
                        if let Some(window) = app_handle.get_webview_window("main") {
                            if let Err(e) = window.set_title(&format!("{base_title} (GPU→CPU)")) {
                                log::warn!(
                                    "[startup] プリウォーム失敗後のウィンドウタイトル設定失敗: {e}"
                                );
                            }
                        }
                    });
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_modules,
            get_attributes,
            capture_status,
            select_character,
            optimize,
            reload_from_dump,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            // 終了時に WinDivert を確実に解放する（recv を中断→ハンドルを閉じる→
            // ドライバサービスを停止/削除）。これを怠ると .sys が残存ロックし、
            // 次回起動やビルド（.sys のコピー）を妨げる。
            if let tauri::RunEvent::Exit = event {
                cleanup_windivert();
            }
        });
}

/// 終了時の WinDivert 解放。Windows 以外では no-op。
fn cleanup_windivert() {
    #[cfg(target_os = "windows")]
    {
        use bpsr_core::capture::windivert;
        windivert::request_shutdown();
        // recv ループがハンドルを閉じるのを最大 ~1s 待つ。
        for _ in 0..50 {
            if windivert::is_handle_closed() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // 共有 "WinDivert" サービスは他アプリと共用するため削除しない（善良な利用者）。
        // dev ビルドのみ、ドライバを STOP して `.sys` ロックを解放する（release は no-op）。
        windivert::stop_driver_for_dev();
        log::info!("WinDivert capture を停止しました");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_dump_json() -> String {
        serde_json::json!([
            {
                "key": 1,
                "uuid": 100,
                "config_id": 5500101,
                "name": "テストモジュール",
                "quality": 3,
                "parts": [
                    { "attr_id": 1110, "attr_name": "力量提升", "value": 10 }
                ]
            }
        ])
        .to_string()
    }

    #[test]
    fn dump_path_for_char_zero_returns_legacy_path() {
        let dir = PathBuf::from("C:/dummy");
        assert_eq!(dump_path_for(&dir, 0), legacy_dump_path(&dir));
        assert_eq!(dump_path_for(&dir, 42), dir.join("owned_modules_42.json"));
    }

    #[test]
    fn scan_character_dump_files_ignores_legacy_file_and_sorts_by_char_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("owned_modules.json"), "[]").unwrap();
        std::fs::write(dir.path().join("owned_modules_5.json"), "[]").unwrap();
        std::fs::write(dir.path().join("owned_modules_2.json"), "[]").unwrap();
        std::fs::write(dir.path().join("not_a_dump.json"), "[]").unwrap();

        let found = scan_character_dump_files(dir.path());
        let ids: Vec<i64> = found.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids,
            vec![2, 5],
            "レガシーファイルを混同せず char_id 昇順で返す"
        );
    }

    #[test]
    fn characters_index_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let chars = vec![
            CharacterEntry {
                char_id: 7,
                name: "七番".to_string(),
                last_update_ms: Some(123),
                source: "capture".to_string(),
                modules: vec![],
            },
            CharacterEntry {
                char_id: 0,
                name: String::new(),
                last_update_ms: None,
                source: "dump".to_string(),
                modules: vec![],
            },
        ];
        let entries = characters_index_from(&chars);
        let path = characters_index_path(dir.path());
        write_characters_index(&path, &entries).expect("write");
        let loaded = load_characters_index(&path).expect("load");
        assert_eq!(loaded, entries);
        assert_eq!(loaded[0].char_id, "7");
        assert_eq!(loaded[1].char_id, "0");
    }

    #[test]
    fn load_at_startup_env_dump_sets_env_fixed_and_char_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dump_path = dir.path().join("custom_dump.json");
        std::fs::write(&dump_path, sample_dump_json()).unwrap();

        let shared = state::new();
        load_at_startup_from(&shared, dir.path(), Some(dump_path));

        let s = shared.lock().unwrap();
        assert!(
            s.env_fixed,
            "BPSR_MODULE_DUMP 指定時は env_fixed を確定させる"
        );
        assert_eq!(s.effective_char_id(), Some(0));
        assert_eq!(s.chars[0].modules.len(), 1);
    }

    #[test]
    fn load_at_startup_falls_through_to_character_files_when_env_path_missing() {
        // BPSR_MODULE_DUMP が設定ミスで存在しないパスを指していても、キャラ別
        // ダンプの読込を丸ごとスキップせずフォールスルーする（設定ミス1つで
        // 事前読込全体が失われるのを防ぐ）。env_fixed 自体は変数が設定されている
        // 限り確定させる。
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("owned_modules_9.json"), sample_dump_json()).unwrap();
        let missing_env_path = dir.path().join("does_not_exist.json");

        let shared = state::new();
        load_at_startup_from(&shared, dir.path(), Some(missing_env_path));

        let s = shared.lock().unwrap();
        assert!(s.env_fixed);
        assert_eq!(s.chars.len(), 1);
        assert_eq!(s.chars[0].char_id, 9);
        assert_eq!(s.chars[0].modules.len(), 1);
    }

    #[test]
    fn load_at_startup_prefers_character_files_and_restores_index_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("owned_modules_2.json"), sample_dump_json()).unwrap();
        std::fs::write(dir.path().join("owned_modules_5.json"), sample_dump_json()).unwrap();
        let idx = vec![
            CharacterIndexEntry {
                char_id: "5".to_string(),
                name: "五番".to_string(),
                last_update_ms: 42,
                module_count: 1,
            },
            CharacterIndexEntry {
                char_id: "2".to_string(),
                name: "二番".to_string(),
                last_update_ms: 7,
                module_count: 1,
            },
        ];
        write_characters_index(&characters_index_path(dir.path()), &idx).unwrap();

        let shared = state::new();
        load_at_startup_from(&shared, dir.path(), None);

        let s = shared.lock().unwrap();
        assert_eq!(s.chars.len(), 2);
        assert_eq!(
            s.chars[0].char_id, 5,
            "characters.json の順序（過去の検知順）を尊重する"
        );
        assert_eq!(s.chars[0].name, "五番");
        assert_eq!(s.chars[0].last_update_ms, Some(42));
        assert_eq!(s.chars[1].char_id, 2);
        assert_eq!(s.chars[1].name, "二番");
    }

    #[test]
    fn load_at_startup_falls_back_to_file_mtime_when_index_last_update_is_unknown() {
        // characters.json 側の last_update_ms == 0（CharacterIndexEntry 生成時の
        // unwrap_or(0)＝不明を表す値）をそのまま採用すると、UI が 1970 年起点の
        // 巨大な経過時間を表示しうる。0 の場合はファイルの mtime へフォールバックする。
        let dir = tempfile::tempdir().expect("tempdir");
        let dump_path = dir.path().join("owned_modules_3.json");
        std::fs::write(&dump_path, sample_dump_json()).unwrap();
        let idx = vec![CharacterIndexEntry {
            char_id: "3".to_string(),
            name: "三番".to_string(),
            last_update_ms: 0,
            module_count: 1,
        }];
        write_characters_index(&characters_index_path(dir.path()), &idx).unwrap();

        let shared = state::new();
        load_at_startup_from(&shared, dir.path(), None);

        let s = shared.lock().unwrap();
        let expected_mtime = file_mtime_ms(&dump_path);
        assert!(
            expected_mtime.is_some(),
            "テスト前提: mtime が取得できること"
        );
        assert_eq!(s.chars[0].last_update_ms, expected_mtime);
        assert_ne!(
            s.chars[0].last_update_ms,
            Some(0),
            "last_update_ms==0（不明）をそのまま採用してはいけない"
        );
    }

    #[test]
    fn load_at_startup_falls_back_to_legacy_dump_when_no_character_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("owned_modules.json"), sample_dump_json()).unwrap();

        let shared = state::new();
        load_at_startup_from(&shared, dir.path(), None);

        let s = shared.lock().unwrap();
        assert_eq!(s.chars.len(), 1);
        assert_eq!(s.chars[0].char_id, 0);
        assert_eq!(s.chars[0].name, "");
        assert_eq!(s.chars[0].source, "dump");
        assert!(!s.env_fixed);
    }

    #[test]
    fn load_at_startup_legacy_then_live_capture_becomes_effective() {
        // レガシー owned_modules.json（char_id=0 の擬似キャラ）だけの状態から起動し、
        // その後に実キャラをライブ検知した状況を模す。擬似キャラが居座らず実キャラが
        // 有効になること（回帰したバグの再発防止）。
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("owned_modules.json"), sample_dump_json()).unwrap();

        let shared = state::new();
        load_at_startup_from(&shared, dir.path(), None);
        assert_eq!(shared.lock().unwrap().effective_char_id(), Some(0));

        {
            let mut s = shared.lock().unwrap();
            let entry = s.upsert_char(123);
            entry.modules = vec![];
            entry.source = "capture".to_string();
            entry.name = "実キャラ".to_string();
        }

        let s = shared.lock().unwrap();
        assert_eq!(
            s.effective_char_id(),
            Some(123),
            "レガシー擬似キャラより実キャラのライブ検知が優先されること"
        );
    }
}
