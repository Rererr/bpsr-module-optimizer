//! モジュール属性・種別のメタデータ。名称はゲームのローカライズデータ
//! (extracted_game_data/localization_all.json) 由来の日本語正式名。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AttrMeta {
    pub id: i32,
    pub name: String,
    /// 上位属性（極・/Ultra）かどうか。
    pub special: bool,
    /// UI グルーピング用カテゴリ。
    pub group: String,
}

/// (id, 日本語名, special, group)
const ATTRS: &[(i32, &str, bool, &str)] = &[
    (1110, "筋力強化", false, "Basic"),
    (1111, "敏捷強化", false, "Basic"),
    (1112, "知力強化", false, "Basic"),
    (1113, "特攻ダメージ強化", false, "Combat"),
    (1114, "精鋭打撃", false, "Combat"),
    (1205, "特攻回復強化", false, "Support"),
    (1206, "マスタリー回復強化", false, "Support"),
    (1307, "魔法耐性", false, "Resist"),
    (1308, "物理耐性", false, "Resist"),
    (1407, "集中・詠唱", false, "Focus"),
    (1408, "集中・攻撃速度", false, "Focus"),
    (1409, "集中・会心", false, "Focus"),
    (1410, "集中・幸運", false, "Focus"),
    (2104, "極・ダメージ増強", true, "Ultra"),
    (2105, "極・適応力", true, "Ultra"),
    (2204, "極・HP凝縮", true, "Ultra"),
    (2205, "極・応急処置", true, "Ultra"),
    (2304, "極・絶境守護", true, "Ultra"),
    (2404, "極・HP変動", true, "Ultra"),
    (2405, "極・HP吸収", true, "Ultra"),
    (2406, "極・幸運会心", true, "Ultra"),
];

/// (config_id, モジュール種別の日本語名)
const MODULES: &[(i32, &str)] = &[
    (5500101, "基本攻撃型モジュール"),
    (5500102, "高性能攻撃型モジュール"),
    (5500103, "卓越攻撃型モジュール"),
    (5500104, "卓越攻撃型モジュール（選択）"),
    (5500201, "基本支援型モジュール"),
    (5500202, "高性能支援型モジュール"),
    (5500203, "卓越支援型モジュール"),
    (5500204, "卓越支援型モジュール（選択）"),
    (5500301, "基本防御型モジュール"),
    (5500302, "高性能防御型モジュール"),
    (5500303, "卓越防御型モジュール"),
    (5500304, "卓越防御型モジュール（選択）"),
];

/// 全属性メタを返す（UI のマルチセレクト用）。
pub fn all() -> Vec<AttrMeta> {
    ATTRS
        .iter()
        .map(|&(id, name, special, group)| AttrMeta {
            id,
            name: name.to_string(),
            special,
            group: group.to_string(),
        })
        .collect()
}

/// 属性ID → 日本語名（未知は None）。
pub fn attr_name(attr_id: i32) -> Option<&'static str> {
    ATTRS
        .iter()
        .find(|&&(id, ..)| id == attr_id)
        .map(|&(_, name, ..)| name)
}

/// config_id → モジュール種別の日本語名（未知は None）。
pub fn module_name(config_id: i32) -> Option<&'static str> {
    MODULES
        .iter()
        .find(|&&(id, _)| id == config_id)
        .map(|&(_, name)| name)
}
