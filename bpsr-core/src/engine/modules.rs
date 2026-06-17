//! 所持モジュール抽出（WorldEnterSnapshot の PlayerSnapshot から復元）。
//!
//! データ経路:
//! - `PlayerSnapshot.item_package.packages[].items[key]` … バッグ内アイテム。
//!   `mod_new_attr.mod_parts` を持つものがモジュール。`mod_parts[i]` = i 番目の属性ID。
//! - `PlayerSnapshot.r#mod.mod_infos[key].init_link_nums[i]` … 同 key・同 i の属性値。
//! - item の `config_id` / `uuid` / `quality` … モジュール種別・固有ID・品質。
//!
//! 現状は実機ダンプ検証用。`dump_modules` が WorldEnterSnapshot 受信時に
//! ログ＋JSON ファイルへ書き出す（出力先は env `BPSR_MODULE_DUMP`、既定は temp）。

use crate::protocol::pb;
use log::{info, warn};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ModulePart {
    pub attr_id: i32,
    pub attr_name: &'static str,
    pub value: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleInfo {
    /// items マップのキー。mod_infos との突合に使う。
    pub key: i64,
    pub uuid: i64,
    pub config_id: i32,
    pub name: &'static str,
    pub quality: i32,
    pub parts: Vec<ModulePart>,
}

/// PlayerSnapshot から所持モジュールを復元する。
/// config_id が未知のものも除外せず含める（検証目的のため網羅優先）。
pub fn parse_modules(snapshot: &pb::PlayerSnapshot) -> Vec<ModuleInfo> {
    let Some(mod_set) = snapshot.r#mod.as_ref() else {
        return Vec::new();
    };
    let Some(item_package) = snapshot.item_package.as_ref() else {
        return Vec::new();
    };

    let mut modules = Vec::new();
    for package in item_package.packages.values() {
        for (key, item) in &package.items {
            let Some(attr) = item.mod_new_attr.as_ref() else {
                continue;
            };
            if attr.mod_parts.is_empty() {
                continue; // モジュール以外のアイテム
            }

            // 属性値は mod_infos[key].init_link_nums に同順で並ぶ。短い方に合わせて zip。
            let link_nums = mod_set
                .mod_infos
                .get(key)
                .map(|e| e.init_link_nums.as_slice())
                .unwrap_or(&[]);
            let n = attr.mod_parts.len().min(link_nums.len());

            let parts = (0..n)
                .map(|i| {
                    let attr_id = attr.mod_parts[i];
                    ModulePart {
                        attr_id,
                        attr_name: attr_name(attr_id),
                        value: link_nums[i],
                    }
                })
                .collect();

            modules.push(ModuleInfo {
                key: *key,
                uuid: item.uuid,
                config_id: item.config_id,
                name: module_name(item.config_id),
                quality: item.quality,
                parts,
            });
        }
    }
    modules
}

/// WorldEnterSnapshot 受信時に所持モジュールをログ＋JSON へダンプする（検証用）。
pub fn dump_modules(snapshot: &pb::PlayerSnapshot) {
    let modules = parse_modules(snapshot);
    if modules.is_empty() {
        info!(
            "[modules] WorldEnterSnapshot 受信。所持モジュール 0 件（item_package/mod 未含 or モジュール無し）"
        );
        return;
    }

    info!("[modules] 所持モジュール {} 件を検出", modules.len());
    for m in &modules {
        let parts: Vec<String> = m
            .parts
            .iter()
            .map(|p| format!("{}({})={}", p.attr_name, p.attr_id, p.value))
            .collect();
        info!(
            "[modules]  cfg={} {} 品質={} uuid={} key={} [{}]",
            m.config_id,
            m.name,
            m.quality,
            m.uuid,
            m.key,
            parts.join(", ")
        );
    }

    let path = std::env::var_os("BPSR_MODULE_DUMP")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("bpsr_owned_modules.json"));

    match serde_json::to_string_pretty(&modules) {
        Ok(json) => match std::fs::write(&path, json) {
            Ok(()) => info!("[modules] ダンプ書き出し成功: {}", path.display()),
            Err(e) => warn!("[modules] ダンプ書き出し失敗 {}: {e}", path.display()),
        },
        Err(e) => warn!("[modules] JSON 直列化失敗: {e}"),
    }
}

/// モジュール種別名（config_id → 名称）。未知は "Unknown"。
/// 名称はリファレンス実装由来の英語ラベル（正典の日本語名は別途要確認）。
fn module_name(config_id: i32) -> &'static str {
    match config_id {
        5500101 => "Basic Attack",
        5500102 => "High-Perf Attack",
        5500103 => "Superior Attack",
        5500104 => "Superior Attack-Select",
        5500201 => "Basic Healing",
        5500202 => "High-Perf Healing",
        5500203 => "Superior Support",
        5500204 => "Superior Support-Select",
        5500301 => "Basic Defense",
        5500302 => "High-Perf Guardian",
        5500303 => "Superior Guardian",
        5500304 => "Superior Guardian-Select",
        _ => "Unknown",
    }
}

/// モジュール属性名（attr_id → 名称）。未知は "Unknown"。
fn attr_name(attr_id: i32) -> &'static str {
    match attr_id {
        1110 => "STR Boost",
        1111 => "DEX Boost",
        1112 => "INT Boost",
        1113 => "Special ATK Damage",
        1114 => "Elite Strike",
        1205 => "Special Heal Boost",
        1206 => "Spec Heal Boost",
        1307 => "Magic Resist",
        1308 => "Physical Resist",
        1407 => "Cast Focus",
        1408 => "ATK Speed Focus",
        1409 => "Crit Focus",
        1410 => "Luck Focus",
        2104 => "Ultra-Damage Stack",
        2105 => "Ultra-Agile Movement",
        2204 => "Ultra-Life Condense",
        2205 => "Ultra-First Aid",
        2304 => "Ultra-Last Stand",
        2404 => "Ultra-Life Surge",
        2405 => "Ultra-Life Drain",
        2406 => "Ultra-Team Luck Crit",
        _ => "Unknown",
    }
}
