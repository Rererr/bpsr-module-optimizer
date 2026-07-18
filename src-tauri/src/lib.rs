//! bpsr-module-optimizer: 所持モジュールから、Lv6数 → Lv5数 → レベル合計 →
//! リンク効果 の優先度で最良の枠（4または5）の組み合わせを求める Tauri アプリ。

mod attrs;
mod capture;
mod elevation;
mod optimizer;
#[cfg(feature = "gpu")]
mod optimizer_gpu;
mod state;

use attrs::AttrMeta;
use optimizer::{Module, OptimizeResult};
use serde::{Deserialize, Serialize};
use state::SharedState;
use std::path::PathBuf;

/// キャプチャ／取得状態のスナップショット。
#[derive(Serialize)]
struct StatusDto {
    /// "init" | "running" | "failed"
    capture_state: String,
    module_count: usize,
    last_update_ms: Option<u64>,
    /// "capture" | "dump" | "none"
    source: String,
    /// ゲームパケットを最後に観測してからの経過 ms（None=未観測）。
    last_game_packet_ms_ago: Option<u64>,
}

/// owned_modules.json（bpsr-checker のダンプ）読込用の形。
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

fn load_dump(path: &PathBuf) -> Result<Vec<Module>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("読込失敗 {}: {e}", path.display()))?;
    let dump: Vec<DumpModule> =
        serde_json::from_str(&text).map_err(|e| format!("JSON 解析失敗: {e}"))?;
    Ok(dump.into_iter().map(Module::from).collect())
}

/// モジュールを JSON でパスへ書き出す（取得時の自動保存で使用）。
/// 出力は `DumpModule` が読み戻せる形（`category` 余剰フィールドは読込側で無視される）。
pub(crate) fn write_dump(path: &PathBuf, modules: &[Module]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(modules).map_err(|e| format!("JSON 変換失敗: {e}"))?;
    std::fs::write(path, json).map_err(|e| format!("保存失敗 {}: {e}", path.display()))
}

/// 既定のダンプパス。env `BPSR_MODULE_DUMP` 優先、なければ exe と同じディレクトリの
/// `owned_modules.json`（存在しなければ事前読込はスキップされ、ライブ取得のみで動作する）。
pub(crate) fn default_dump_path() -> PathBuf {
    if let Some(p) = std::env::var_os("BPSR_MODULE_DUMP") {
        return PathBuf::from(p);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|dir| dir.join("owned_modules.json")))
        .unwrap_or_else(|| PathBuf::from("owned_modules.json"))
}

// ---- コマンド ----

#[tauri::command]
fn get_modules(state: tauri::State<SharedState>) -> Vec<Module> {
    state.lock().expect("state poisoned").modules.clone()
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
    StatusDto {
        capture_state,
        module_count: s.modules.len(),
        last_update_ms: s.last_update_ms,
        source: s.source.clone(),
        last_game_packet_ms_ago: last_game,
    }
}

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
) -> Result<OptimizeResult, String> {
    // MutexGuard を await をまたいで保持しないよう、先にモジュールを複製する。
    let modules = state.lock().expect("state poisoned").modules.clone();
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
        )
    })
    .await
    .map_err(|e| format!("最適化タスク失敗: {e}"))?
}

/// 指定パス（省略時は既定）のダンプを読み込み、現在のモジュールを差し替える。
#[tauri::command]
fn reload_from_dump(
    state: tauri::State<SharedState>,
    path: Option<String>,
) -> Result<usize, String> {
    let p = path.map(PathBuf::from).unwrap_or_else(default_dump_path);
    let modules = load_dump(&p)?;
    let count = modules.len();
    let mut s = state.lock().expect("state poisoned");
    s.modules = modules;
    s.last_update_ms = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    );
    s.source = "dump".to_string();
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
            // 起動時: 既定ダンプがあれば事前読込（キャプチャ前でも UI に表示できる）。
            let p = default_dump_path();
            if p.exists() {
                match load_dump(&p) {
                    Ok(modules) => {
                        let count = modules.len();
                        if let Ok(mut s) = shared.lock() {
                            s.modules = modules;
                            s.source = "dump".to_string();
                        }
                        log::info!("[startup] ダンプから {count} 件を事前読込: {}", p.display());
                    }
                    Err(e) => log::warn!("[startup] ダンプ事前読込スキップ: {e}"),
                }
            }
            // ライブキャプチャ開始（要管理者権限）。
            capture::spawn(app.handle().clone(), shared.clone());

            // GPU版ビルドはウィンドウタイトルで区別できるようにする（失敗しても起動は続行）。
            #[cfg(feature = "gpu")]
            {
                use tauri::Manager;
                if let Some(window) = app.get_webview_window("main") {
                    let title = window.title().unwrap_or_default();
                    if let Err(e) = window.set_title(&format!("{title} (GPU)")) {
                        log::warn!("[startup] ウィンドウタイトル設定失敗: {e}");
                    }
                }
                // GPUデバイス初期化・パイプラインコンパイルを先食いしておく（初回クエリの
                // +0.2〜1.5s を隠す）。探索クエリを塞がないよう別スレッドで実行する
                // （optimize コマンドと同じ optimizer_gpu::gpu_context の OnceLock を叩くだけ）。
                std::thread::spawn(optimizer_gpu::prewarm);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_modules,
            get_attributes,
            capture_status,
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
