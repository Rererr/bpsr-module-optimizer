//! ライブキャプチャ連携。bpsr-core の WinDivert キャプチャを起動し、
//! WorldEnterSnapshot だけを拾って所持モジュールを抽出・共有状態へ反映する。
//! 抽出のたびに `modules-updated` イベントをフロントへ発火する。

use crate::optimizer::Module;
use crate::state::SharedState;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
            {
                let mut s = state.lock().expect("state poisoned");
                s.modules = modules;
                s.last_update_ms = Some(now_ms());
                s.source = "capture".to_string();
            }
            log::info!("[capture] 所持モジュール {count} 件を更新");
            let _ = app.emit("modules-updated", count);
        }
        log::warn!("[capture] receiver closed");
    });
}

/// 非 Windows ではキャプチャ不可。
#[cfg(not(target_os = "windows"))]
pub fn spawn(_app: tauri::AppHandle, _state: SharedState) {
    log::warn!("[capture] パケットキャプチャは Windows 専用です");
}
