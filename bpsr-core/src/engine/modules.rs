//! 所持モジュール抽出（WorldEnterSnapshot の PlayerSnapshot から復元）。
//!
//! データ経路:
//! - `PlayerSnapshot.item_package.packages[].items[key]` … バッグ内アイテム。
//!   `mod_new_attr.mod_parts` を持つものがモジュール。`mod_parts[i]` = i 番目の属性ID。
//! - item の `mod_part_values.values[attr_id]`（InvItem の field 21）… 属性値。
//!   2026-10-29 のゲーム更新で導入。並びは `mod_parts` と一致しないので attr_id で引く。
//! - `PlayerSnapshot.r#mod.mod_infos[key].init_link_nums[i]` … 更新前の属性値（同 key・同 i）。
//!   更新後は送られないが、`mod_part_values` が無いときのフォールバックとして残す。
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
    let Some(item_package) = snapshot.item_package.as_ref() else {
        return Vec::new();
    };
    let mod_infos = snapshot.r#mod.as_ref().map(|m| &m.mod_infos);

    let mut modules = Vec::new();
    for package in item_package.packages.values() {
        for (key, item) in &package.items {
            let Some(attr) = item.mod_new_attr.as_ref() else {
                continue;
            };
            if attr.mod_parts.is_empty() {
                continue; // モジュール以外のアイテム
            }

            let parts = match item.mod_part_values.as_ref() {
                // 2026-10-29 のゲーム更新以降: 値は item 自身に attr_id → 値 で載る。
                // 並びは mod_parts と一致しないため、位置ではなく attr_id で引く。
                Some(pv) if !pv.values.is_empty() => attr
                    .mod_parts
                    .iter()
                    .filter_map(|&attr_id| {
                        let value = pv.values.get(&attr_id).copied();
                        if value.is_none() {
                            warn!(
                                "[modules] key={key}: 属性 {attr_id} の値が mod_part_values に無い（この属性を除外）"
                            );
                        }
                        value.map(|value| ModulePart {
                            attr_id,
                            attr_name: attr_name(attr_id),
                            value,
                        })
                    })
                    .collect(),
                // 更新前の形式: 値は mod_infos[key].init_link_nums に mod_parts と同順で並ぶ。
                // 短い方に合わせて zip。
                _ => {
                    let link_nums = mod_infos
                        .and_then(|m| m.get(key))
                        .map(|e| e.init_link_nums.as_slice())
                        .unwrap_or(&[]);
                    attr.mod_parts
                        .iter()
                        .zip(link_nums)
                        .map(|(&attr_id, &value)| ModulePart {
                            attr_id,
                            attr_name: attr_name(attr_id),
                            value,
                        })
                        .collect()
                }
            };

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn snapshot_with(item: pb::InvItem, key: i64, r#mod: Option<pb::ModuleSet>) -> pb::PlayerSnapshot {
        let bag = pb::InvBag {
            items: HashMap::from([(key, item)]),
        };
        pb::PlayerSnapshot {
            item_package: Some(pb::InvBags {
                packages: HashMap::from([(1, bag)]),
            }),
            r#mod,
            ..Default::default()
        }
    }

    fn module_item(mod_parts: Vec<i32>, values: Option<HashMap<i32, i32>>) -> pb::InvItem {
        pb::InvItem {
            uuid: 100,
            config_id: 5500102,
            quality: 3,
            mod_new_attr: Some(pb::ModuleAttrs { mod_parts }),
            mod_part_values: values.map(|values| pb::ModulePartValues { values }),
        }
    }

    fn values_of(m: &ModuleInfo) -> Vec<(i32, i32)> {
        m.parts.iter().map(|p| (p.attr_id, p.value)).collect()
    }

    #[test]
    fn reads_values_from_item_by_attr_id_after_update() {
        // mod_part_values の並びは mod_parts と無関係（実機でも順序が食い違う）。
        let item = module_item(
            vec![1409, 1307, 1308],
            Some(HashMap::from([(1308, 3), (1409, 7), (1307, 5)])),
        );
        // 更新後の ModuleSet は mod_infos を送らない。
        let mods = parse_modules(&snapshot_with(item, 100, Some(pb::ModuleSet::default())));
        assert_eq!(mods.len(), 1);
        assert_eq!(values_of(&mods[0]), vec![(1409, 7), (1307, 5), (1308, 3)]);
    }

    #[test]
    fn reads_values_without_module_set() {
        let item = module_item(vec![1111], Some(HashMap::from([(1111, 10)])));
        let mods = parse_modules(&snapshot_with(item, 100, None));
        assert_eq!(values_of(&mods[0]), vec![(1111, 10)]);
    }

    #[test]
    fn drops_attr_missing_from_values() {
        let item = module_item(vec![1111, 1408], Some(HashMap::from([(1408, 4)])));
        let mods = parse_modules(&snapshot_with(item, 100, None));
        assert_eq!(values_of(&mods[0]), vec![(1408, 4)]);
    }

    #[test]
    fn falls_back_to_mod_infos_for_old_format() {
        let item = module_item(vec![1111, 1408], None);
        let set = pb::ModuleSet {
            mod_infos: HashMap::from([(
                100,
                pb::ModuleEntry {
                    init_link_nums: vec![6, 2],
                },
            )]),
        };
        let mods = parse_modules(&snapshot_with(item, 100, Some(set)));
        assert_eq!(values_of(&mods[0]), vec![(1111, 6), (1408, 2)]);
    }
}
