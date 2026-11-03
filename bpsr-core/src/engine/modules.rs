//! 所持モジュール抽出（WorldEnterSnapshot の PlayerSnapshot から復元）。
//!
//! データ経路:
//! - `PlayerSnapshot.item_package.packages[].items[key]` … バッグ内アイテム。
//!   `mod_new_attr.mod_parts` を持つものがモジュール。`mod_parts[i]` = i 番目の属性ID。
//! - item の `mod_part_values.values[attr_id]`（InvItem の field 21）… 属性値。
//!   2026-08-31〜11-04 の間のゲーム更新で導入（正確な配信日は未確認）。並びは
//!   `mod_parts` と一致しないので attr_id で引く。
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
    /// items マップのキー。更新前の形式では mod_infos との突合に使う。
    pub key: i64,
    pub uuid: i64,
    pub config_id: i32,
    pub name: &'static str,
    pub quality: i32,
    pub parts: Vec<ModulePart>,
}

/// 属性値の取得で起きた異常。ゲーム更新で値の置き場が変わったことを呼び出し側
/// （UI への通知など）が知るために返す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueIssue {
    /// 一部のモジュールで属性値の一部または全部が取れなかった。
    /// 該当モジュールも値の取れた属性だけで `modules` に含まれる。
    Partial { affected: usize, total: usize },
    /// 全モジュールで属性値が1つも取れなかった。`modules` は空になる。
    Missing { total: usize },
}

/// [`parse_modules`] の結果。
#[derive(Debug, Clone, Default)]
pub struct ParsedModules {
    pub modules: Vec<ModuleInfo>,
    /// 属性値が全部取れたら None。
    pub value_issue: Option<ValueIssue>,
}

/// PlayerSnapshot から所持モジュールを復元する。
/// config_id が未知のものも除外せず含める（検証目的のため網羅優先）。
///
/// ただし全モジュールで属性値が1つも取れないときは `modules` を空にし、
/// `value_issue` を `Missing` にして返す。属性が空のモジュールを返すと、呼び出し側が
/// 保存済みの正しいデータを空で上書きし、全件スコア0の再探索まで走らせてしまう
/// （Issue #1 の壊れ方）。呼び出し側は `Missing` のとき、保持中のモジュール一覧と
/// 保存データに触れない（警告の記録だけにする）ことで、最後に正しく取れたデータを残せる。
pub fn parse_modules(snapshot: &pb::PlayerSnapshot) -> ParsedModules {
    let Some(item_package) = snapshot.item_package.as_ref() else {
        return ParsedModules::default();
    };
    let mod_infos = snapshot.r#mod.as_ref().map(|m| &m.mod_infos);

    let mut modules = Vec::new();
    // 値が一部でも取れなかったモジュール数（形式ごと）。ゲーム更新で値の置き場が
    // また変わったときにログから気付けるよう、最後にまとめて警告する。
    let mut incomplete_new = 0usize;
    let mut incomplete_old = 0usize;
    for package in item_package.packages.values() {
        for (key, item) in &package.items {
            let Some(attr) = item.mod_new_attr.as_ref() else {
                continue;
            };
            if attr.mod_parts.is_empty() {
                continue; // モジュール以外のアイテム
            }

            let parts: Vec<ModulePart> = match item.mod_part_values.as_ref() {
                // 上記のゲーム更新以降: 値は item 自身に attr_id → 値 で載る。
                // 並びは mod_parts と一致しないため、位置ではなく attr_id で引く。
                // 値の無い属性は除外する（更新前の zip による切り詰めと同じ扱い）。
                Some(pv) if !pv.values.is_empty() => {
                    let parts: Vec<ModulePart> = attr
                        .mod_parts
                        .iter()
                        .filter_map(|&attr_id| {
                            pv.values.get(&attr_id).map(|&value| ModulePart {
                                attr_id,
                                attr_name: attr_name(attr_id),
                                value,
                            })
                        })
                        .collect();
                    if parts.len() < attr.mod_parts.len() {
                        incomplete_new += 1;
                    }
                    parts
                }
                // 更新前の形式: 値は mod_infos[key].init_link_nums に mod_parts と同順で並ぶ。
                // 短い方に合わせて zip。
                _ => {
                    let link_nums = mod_infos
                        .and_then(|m| m.get(key))
                        .map(|e| e.init_link_nums.as_slice())
                        .unwrap_or(&[]);
                    if link_nums.len() < attr.mod_parts.len() {
                        incomplete_old += 1;
                    }
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

    let total = modules.len();
    let incomplete = incomplete_new + incomplete_old;
    if incomplete > 0 {
        warn!(
            "[modules] 属性値の一部または全部が取れないモジュールがある: {incomplete} / {total} 件（mod_part_values 経由 {incomplete_new} 件・mod_infos 経由 {incomplete_old} 件）。ゲーム更新で値の置き場が変わった可能性"
        );
    }
    // 全モジュールで値が1つも取れないなら、値の置き場が変わったと見なして破棄する
    // （理由は関数の doc 参照）。
    if total > 0 && modules.iter().all(|m| m.parts.is_empty()) {
        warn!("[modules] モジュール {total} 件すべてで属性値が取れないため、取得結果を破棄します");
        return ParsedModules {
            modules: Vec::new(),
            value_issue: Some(ValueIssue::Missing { total }),
        };
    }
    ParsedModules {
        modules,
        value_issue: (incomplete > 0).then_some(ValueIssue::Partial {
            affected: incomplete,
            total,
        }),
    }
}

/// WorldEnterSnapshot 受信時に所持モジュールをログ＋JSON へダンプする（検証用）。
pub fn dump_modules(snapshot: &pb::PlayerSnapshot) {
    let modules = parse_modules(snapshot).modules;
    if modules.is_empty() {
        info!(
            "[modules] WorldEnterSnapshot 受信。所持モジュール 0 件（item_package 未含 or モジュール無し or 属性値が全件取れない）"
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

    fn snapshot_of(items: Vec<(i64, pb::InvItem)>, r#mod: Option<pb::ModuleSet>) -> pb::PlayerSnapshot {
        let bag = pb::InvBag {
            items: items.into_iter().collect(),
        };
        pb::PlayerSnapshot {
            item_package: Some(pb::InvBags {
                packages: HashMap::from([(1, bag)]),
            }),
            r#mod,
            ..Default::default()
        }
    }

    /// key=100 の item 1件だけを持つスナップショットを解析する。
    fn parse_single(item: pb::InvItem, r#mod: Option<pb::ModuleSet>) -> ParsedModules {
        parse_modules(&snapshot_of(vec![(100, item)], r#mod))
    }

    /// 1件だけ返ったことを確かめてから、その属性を (attr_id, 値) で返す。
    /// 0件だと「値が decode されず全件破棄された」ことを示すので、index の panic ではなく
    /// 原因の分かるメッセージで落とす。
    fn single_values(parsed: &ParsedModules) -> Vec<(i32, i32)> {
        assert_eq!(
            parsed.modules.len(),
            1,
            "モジュールが1件返らない（値が decode されず全件破棄された可能性）: {:?}",
            parsed.value_issue
        );
        values_of(&parsed.modules[0])
    }

    fn module_item(mod_parts: Vec<i32>, values: Option<HashMap<i32, i32>>) -> pb::InvItem {
        pb::InvItem {
            uuid: 100,
            config_id: 5500102,
            mod_new_attr: Some(pb::ModuleAttrs { mod_parts }),
            mod_part_values: values.map(|values| pb::ModulePartValues { values }),
            ..Default::default()
        }
    }

    // --- protobuf wire 形式の手組み（pb.proto のフィールド番号を prost 非経由で固定する） ---

    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return out;
            }
            out.push(b | 0x80);
        }
    }

    fn field_varint(field: u32, v: u64) -> Vec<u8> {
        let mut out = varint(u64::from(field << 3));
        out.extend(varint(v));
        out
    }

    fn field_bytes(field: u32, body: &[u8]) -> Vec<u8> {
        let mut out = varint(u64::from((field << 3) | 2));
        out.extend(varint(body.len() as u64));
        out.extend_from_slice(body);
        out
    }

    /// 実機（2026-11-04 取得）と同じ形: f13.f1 = packed mod_parts、
    /// f21 = ラッパー、その f1 に {f1: attr_id, f2: 値} の繰り返し。
    fn wire_item(mod_parts: &[i32], values: &[(i32, i32)]) -> Vec<u8> {
        let packed: Vec<u8> = mod_parts.iter().flat_map(|&a| varint(a as u64)).collect();
        let mod_new_attr = field_bytes(1, &packed);
        let wrapper: Vec<u8> = values
            .iter()
            .flat_map(|&(a, v)| {
                let mut entry = field_varint(1, a as u64);
                entry.extend(field_varint(2, v as u64));
                field_bytes(1, &entry)
            })
            .collect();
        let mut item = field_varint(1, 100);
        item.extend(field_varint(2, 5500102));
        item.extend(field_varint(9, 3));
        item.extend(field_bytes(13, &mod_new_attr));
        item.extend(field_bytes(21, &wrapper));
        item
    }

    #[test]
    fn decodes_wire_format_of_updated_item() {
        use prost::Message;
        // f21 の並びを mod_parts と変えておく。prost は map を HashMap に decode するので
        // wire 上の並びは保たれず、位置で引く誤実装が偶然通る確率は 1 回あたり約 1/6
        // ある。decode のたびに HashMap のハッシュ種が変わるので、繰り返して偶然の
        // 通過を潰す（50 回で約 1e-39）。
        let bytes = wire_item(&[1409, 1307, 1308], &[(1308, 3), (1409, 7), (1307, 5)]);
        for _ in 0..50 {
            let item = pb::InvItem::decode(bytes.as_slice()).expect("InvItem の decode に失敗");
            assert_eq!((item.uuid, item.config_id, item.quality), (100, 5500102, 3));
            let parsed = parse_single(item, Some(pb::ModuleSet::default()));
            assert_eq!(single_values(&parsed), vec![(1409, 7), (1307, 5), (1308, 3)]);
            assert_eq!(parsed.value_issue, None);
        }
    }

    #[test]
    fn prefers_item_values_over_mod_infos() {
        let item = module_item(vec![1111], Some(HashMap::from([(1111, 9)])));
        let set = pb::ModuleSet {
            mod_infos: HashMap::from([(100, pb::ModuleEntry { init_link_nums: vec![1] })]),
        };
        assert_eq!(single_values(&parse_single(item, Some(set))), vec![(1111, 9)]);
    }

    #[test]
    fn falls_back_when_item_values_are_empty() {
        let item = module_item(vec![1111], Some(HashMap::new()));
        let set = pb::ModuleSet {
            mod_infos: HashMap::from([(100, pb::ModuleEntry { init_link_nums: vec![4] })]),
        };
        assert_eq!(single_values(&parse_single(item, Some(set))), vec![(1111, 4)]);
    }

    #[test]
    fn returns_nothing_when_no_values_anywhere() {
        // Issue #1 の状態: 属性 ID はあるが値の置き場がどこにも無い。
        // 空の属性で保存済みデータを上書きしないよう 0 件を返し、Missing を報告する。
        let item = module_item(vec![1111, 1408], None);
        let parsed = parse_single(item, Some(pb::ModuleSet::default()));
        assert!(parsed.modules.is_empty());
        assert_eq!(parsed.value_issue, Some(ValueIssue::Missing { total: 1 }));
    }

    #[test]
    fn keeps_modules_when_only_some_lack_values() {
        // 破棄は「全件で値が無い」ときだけ。1件でも値が取れていれば全件返し、
        // 値の無いモジュールは属性なしのまま含めて Partial を報告する。
        let ok = module_item(vec![1111], Some(HashMap::from([(1111, 5)])));
        let broken = module_item(vec![1408], None);
        let parsed = parse_modules(&snapshot_of(vec![(100, ok), (200, broken)], None));
        let mut by_key: Vec<(i64, Vec<(i32, i32)>)> =
            parsed.modules.iter().map(|m| (m.key, values_of(m))).collect();
        by_key.sort();
        assert_eq!(by_key, vec![(100, vec![(1111, 5)]), (200, vec![])]);
        assert_eq!(
            parsed.value_issue,
            Some(ValueIssue::Partial { affected: 1, total: 2 })
        );
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
        let parsed = parse_single(item, Some(pb::ModuleSet::default()));
        assert_eq!(single_values(&parsed), vec![(1409, 7), (1307, 5), (1308, 3)]);
    }

    #[test]
    fn reads_values_without_module_set() {
        let item = module_item(vec![1111], Some(HashMap::from([(1111, 10)])));
        assert_eq!(single_values(&parse_single(item, None)), vec![(1111, 10)]);
    }

    #[test]
    fn drops_attr_missing_from_values() {
        let item = module_item(vec![1111, 1408], Some(HashMap::from([(1408, 4)])));
        let parsed = parse_single(item, None);
        assert_eq!(single_values(&parsed), vec![(1408, 4)]);
        assert_eq!(
            parsed.value_issue,
            Some(ValueIssue::Partial { affected: 1, total: 1 })
        );
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
        let parsed = parse_single(item, Some(set));
        assert_eq!(single_values(&parsed), vec![(1111, 6), (1408, 2)]);
        assert_eq!(parsed.value_issue, None);
    }
}
