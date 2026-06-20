//! ライブキャプチャ連携。bpsr-core の WinDivert キャプチャを起動し、
//! WorldEnterSnapshot だけを拾って所持モジュールを抽出・共有状態へ反映する。
//! 抽出のたびに `modules-updated` イベントをフロントへ発火する。

use crate::optimizer::Module;
use crate::state::SharedState;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 自動保存の最小間隔。マップ頻繁移動時のディスク書込を上限で抑えるセーフティ。
const MIN_SAVE_INTERVAL: Duration = Duration::from_secs(5);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 順序非依存・低コストな内容署名。追加/削除・品質・値合計の変化を検知し、
/// 内容が変わらないマップ移動での無駄な保存をスキップするために使う。
fn module_signature(mods: &[Module]) -> u64 {
    let mut sig = mods.len() as u64;
    for m in mods {
        let val_sum: i64 = m.parts.iter().map(|p| p.value as i64).sum();
        sig ^= (m.uuid as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (m.quality as u64).wrapping_mul(0x0100_0000_01B3)
            ^ (val_sum as u64);
    }
    sig
}

/// キャプチャを開始する（Windows のみ実体動作。要管理者権限）。
#[cfg(target_os = "windows")]
pub fn spawn(app: tauri::AppHandle, state: SharedState) {
    use bpsr_core::engine::modules::parse_modules;
    use bpsr_core::protocol::opcodes::Pkt;
    use bpsr_core::protocol::pb;
    use prost::Message;
    use tauri::Emitter;

    tauri::async_runtime::spawn(async move {
        let mut rx = bpsr_core::capture::windivert::start_capture();
        log::info!("[capture] WinDivert capture started");

        // 自動保存のセーフティ用。直近に保存した内容署名と保存時刻を保持する。
        let mut last_sig: Option<u64> = None;
        let mut last_save: Option<Instant> = None;

        while let Some(env) = rx.recv().await {
            if !matches!(env.op, Pkt::WorldEnterSnapshot) {
                continue;
            }
            let msg = match pb::WorldEnterSnapshot::decode(env.data.as_slice()) {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("[capture] WorldEnterSnapshot decode 失敗: {e}");
                    continue;
                }
            };
            let Some(v_data) = msg.v_data else {
                continue;
            };
            let core_mods = parse_modules(&v_data);
            if core_mods.is_empty() {
                continue;
            }

            let modules: Vec<Module> = core_mods.iter().map(Module::from_core).collect();
            let count = modules.len();

            // 自動保存の要否を判定（メモリ更新前に。clone は保存する時だけ）。
            let sig = module_signature(&modules);
            let now = Instant::now();
            let throttled = last_save.is_some_and(|t| now.duration_since(t) < MIN_SAVE_INTERVAL);
            let snapshot = (last_sig != Some(sig) && !throttled).then(|| modules.clone());

            {
                let mut s = state.lock().expect("state poisoned");
                s.modules = modules;
                s.last_update_ms = Some(now_ms());
                s.source = "capture".to_string();
            }
            log::info!("[capture] 所持モジュール {count} 件を更新");
            let _ = app.emit("modules-updated", count);

            // 取得内容を最新1件として自動保存（owned_modules.json を上書き）。
            if let Some(snap) = snapshot {
                match crate::write_dump(&crate::default_dump_path(), &snap) {
                    Ok(()) => {
                        last_sig = Some(sig);
                        last_save = Some(now);
                        log::info!("[capture] 所持モジュール {count} 件を自動保存");
                    }
                    Err(e) => log::warn!("[capture] 自動保存失敗: {e}"),
                }
            }
        }
        log::warn!("[capture] receiver closed");
    });
}

/// 非 Windows ではキャプチャ不可。
#[cfg(not(target_os = "windows"))]
pub fn spawn(_app: tauri::AppHandle, _state: SharedState) {
    log::warn!("[capture] パケットキャプチャは Windows 専用です");
}
