//! ライブキャプチャ連携。bpsr-core の WinDivert キャプチャを起動し、
//! WorldEnterSnapshot だけを拾って所持モジュールを抽出・共有状態へ反映する。
//!
//! WorldEnterSnapshot（PlayerSnapshot）は char_id/char_base.name を含むため、複数
//! クライアント（複数キャラ）が同時に起動していても、どのキャラ由来か単体で判別
//! できる（bpsr-checker のような conn↔char_id 学習は不要）。受信・解析・ファイル
//! 保存は検知した全キャラ分を常時行い、選択（`state::Inner::selected_char_id`）は
//! 表示/探索対象を絞るだけで受信自体には影響しない。
//!
//! 有効キャラ（選択中、無ければ先着）の内容が変わった時だけ `modules-updated`
//! イベントをフロントへ発火する（未選択キャラの受信で勝手に再探索させないため）。
//! 例外として、属性値が全件取れないまま初検知のキャラへ有効キャラが移った時も、
//! payload 0 で発火する（フロントは探索せず古い結果を片付ける）。
//! キャラ一覧（新規検知・名前・件数）が変わった時は `characters-updated` を発火する。

use crate::optimizer::Module;
use crate::state::SharedState;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 自動保存の最小間隔。マップ頻繁移動時のディスク書込を上限で抑えるセーフティ。
const MIN_SAVE_INTERVAL: Duration = Duration::from_secs(5);

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

/// キャラ（char_id）ごとの自動保存スロットリング・再探索イベント発火抑止の状態。
/// 単一変数だとキャラ横断で共有されてしまい、片方のクライアントの受信がもう片方の
/// 保存/再探索通知を抑止してしまうため、char_id ごとに独立管理する。
#[derive(Default)]
struct CharThrottle {
    last_sig: Option<u64>,
    last_save: Option<Instant>,
    last_emitted_sig: Option<u64>,
}

/// キャプチャを開始する（Windows のみ実体動作。要管理者権限）。
#[cfg(target_os = "windows")]
pub fn spawn(app: tauri::AppHandle, state: SharedState) {
    use bpsr_core::engine::modules::{parse_modules, ValueIssue};
    use bpsr_core::protocol::opcodes::Pkt;
    use bpsr_core::protocol::pb;
    use prost::Message;
    use tauri::Emitter;

    tauri::async_runtime::spawn(async move {
        let mut rx = bpsr_core::capture::windivert::start_capture();
        log::info!("[capture] WinDivert capture started");

        let mut throttles: HashMap<i64, CharThrottle> = HashMap::new();
        // characters.json の書込抑止・characters-updated 発火抑止（キャラ一覧の内容署名）。
        let mut last_chars_sig: Option<u64> = None;
        let dir = crate::modules_dir();

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
            let char_id = v_data.char_id;
            // char_id==0 は未ログイン/未特定を示す番兵値（上流 bpsr-core の
            // processor.rs と同じ扱い）。char_id=0 はレガシー単一ファイル/env指定
            // ダンプ由来の擬似キャラ専用に予約しているため、実キャプチャからの
            // 0 はレガシーファイル上書き・幽霊キャラ化を防ぐために弾く。
            if char_id == 0 {
                continue;
            }
            let char_name = v_data
                .char_base
                .as_ref()
                .map(|b| b.name.clone())
                .unwrap_or_default();
            let parsed = parse_modules(&v_data);
            // 属性値が全件取れない（ゲーム更新で値の置き場が変わった等）ときは、
            // モジュール一覧・保存データには触れず、UI へ出す警告だけを記録する。
            // このキャラの前回正しく取れたデータは、そのまま表示・探索に使われ続ける。
            if let Some(ValueIssue::Missing { total }) = parsed.value_issue {
                let effective_changed = state
                    .lock()
                    .expect("state poisoned")
                    .record_value_missing(char_id, &char_name, total);
                // 初検知のキャラへ有効キャラが移った場合は、別キャラのデータで出した
                // 結果がフロントに残らないよう、0 件として通知する。
                if effective_changed {
                    log::info!(
                        "[capture] キャラ{char_id}: 属性値が取れないまま有効キャラが切り替わったため結果のリセットを通知"
                    );
                    let _ = app.emit("modules-updated", 0usize);
                }
                continue;
            }
            if parsed.modules.is_empty() {
                continue;
            }
            let value_issue = parsed.value_issue;

            let modules: Vec<Module> = parsed.modules.iter().map(Module::from_core).collect();
            let count = modules.len();

            // 自動保存・再探索通知の要否を判定（メモリ更新前に。clone は保存する時だけ）。
            let sig = module_signature(&modules);
            let now = Instant::now();
            let throttle = throttles.entry(char_id).or_default();
            let throttled = throttle
                .last_save
                .is_some_and(|t| now.duration_since(t) < MIN_SAVE_INTERVAL);
            let should_save = throttle.last_sig != Some(sig) && !throttled;
            let snapshot = should_save.then(|| modules.clone());
            let should_emit_modules = throttle.last_emitted_sig != Some(sig);

            // 共有状態を更新し、このキャラが有効キャラか・キャラ索引の内容署名を取得する。
            let (is_effective, chars_sig, index_entries) = {
                let mut s = state.lock().expect("state poisoned");
                {
                    let entry = s.upsert_char(char_id);
                    entry.modules = modules;
                    entry.value_issue = value_issue;
                    entry.last_update_ms = Some(crate::now_ms());
                    entry.source = "capture".to_string();
                    if !char_name.is_empty() {
                        entry.name = char_name;
                    }
                }
                let is_effective = s.effective_char_id() == Some(char_id);
                let chars_sig = s.characters_signature();
                let index_entries = crate::characters_index_from(&s.chars);
                (is_effective, chars_sig, index_entries)
            };

            // 有効キャラの内容が変わった時だけ再探索イベントを発火する。未選択キャラの
            // 受信や、装備を変えていない単なるゾーン移動での無駄な自動再探索を避ける。
            // last_emitted_sig は有効/非有効に関わらず記録する。そうしないと、非有効
            // キャラのまま署名だけ古くなり、キャラ切替直後の初回ゾーン移動（内容不変）で
            // 無駄な再探索が走ってしまう（切替直後の反映自体は select_character 側の
            // emit が担保するため、ここで改めて発火する必要は無い）。
            if should_emit_modules {
                throttle.last_emitted_sig = Some(sig);
                if is_effective {
                    log::info!(
                        "[capture] キャラ{char_id}: 所持モジュール {count} 件を更新（再探索を通知）"
                    );
                    let _ = app.emit("modules-updated", count);
                } else {
                    log::debug!(
                        "[capture] キャラ{char_id}: 所持モジュール {count} 件を更新（有効キャラでないため再探索は通知しない）"
                    );
                }
            } else {
                log::debug!("[capture] キャラ{char_id}: 所持モジュール {count} 件（内容変化なし）");
            }

            // 取得内容を最新1件として自動保存（キャラ別ダンプを上書き）。
            if let Some(snap) = snapshot {
                let path = crate::dump_path_for(&dir, char_id);
                match crate::write_dump(&path, &snap) {
                    Ok(()) => {
                        throttle.last_sig = Some(sig);
                        throttle.last_save = Some(now);
                        log::info!(
                            "[capture] キャラ{char_id}: 所持モジュール {count} 件を自動保存"
                        );
                    }
                    Err(e) => log::warn!("[capture] キャラ{char_id}: 自動保存失敗: {e}"),
                }
            }

            // キャラ一覧（新規検知・名前・件数）が変わった時だけ characters.json を書き出し、
            // フロントへキャラ選択 UI 更新のイベントを発火する。
            if last_chars_sig != Some(chars_sig) {
                last_chars_sig = Some(chars_sig);
                let idx_path = crate::characters_index_path(&dir);
                match crate::write_characters_index(&idx_path, &index_entries) {
                    Ok(()) => log::info!("[capture] キャラ索引を更新（{}件）", index_entries.len()),
                    Err(e) => log::warn!("[capture] キャラ索引の保存失敗: {e}"),
                }
                let _ = app.emit("characters-updated", ());
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
