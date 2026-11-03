//! アプリ共有状態: キャラ別の所持モジュールと選択状態。
//!
//! 複数クライアント（複数キャラ）を同時にキャプチャできるため、モジュールは
//! `chars`（検知順）にキャラ別で保持する。表示/探索に使う「有効キャラ」は
//! [`Inner::effective_char`] で以下の優先順に解決する:
//! 1. 明示選択（`selected_char_id`）
//! 2. `env_fixed`（`BPSR_MODULE_DUMP` 指定時の検証用途）なら char_id=0 の擬似キャラ
//! 3. char_id != 0（擬似キャラでない）の先着
//! 4. それも無ければ `chars` の先頭
//!
//! env_fixed をキャプチャ先着より優先するのは、`BPSR_MODULE_DUMP` の指定が
//! 「このダンプを見る」という明示的な検証意図だから。3段目を「実キャラの先着」に
//! するのは、起動時にレガシー `owned_modules.json`（char_id=0 の擬似キャラ）だけが
//! 存在する状態から実キャラを検知した場合に、擬似キャラが先着として居座って実キャラの
//! 更新が反映されない回帰を防ぐため（実キャラの検知は常にレガシー擬似キャラの事前
//! 読込より後に起こる）。
//!
//! 3段目の条件に可変の `source`（"capture" | "dump"）を使わないのは、`source` が
//! 実行中に書き換わる値だから（キャラがゾーン移動して capture 由来になった瞬間や、
//! `reload_from_dump` でダンプ再読込した瞬間に条件が変わり、ユーザー操作なしに
//! 有効キャラが静かに切り替わってしまう回帰があったため）。char_id が 0 かどうかは
//! 不変な区分（擬似キャラの予約値）なので、これだけを条件にすることで実行中に
//! 結果が変わらない安定した解決規則になる。

use crate::optimizer::Module;
use std::sync::{Arc, Mutex};

/// 検知済み1キャラ分の状態。
#[derive(Default)]
pub struct CharacterEntry {
    pub char_id: i64,
    /// 空文字なら未特定（レガシー単一ファイル/env指定ダンプ由来の擬似キャラ）。
    pub name: String,
    /// 最後にモジュールを更新した unix ms（キャプチャ or ダンプ読込）。
    pub last_update_ms: Option<u64>,
    /// モジュールの取得元（"capture" | "dump" | "none"）。"none" は属性値が読めないまま
    /// 検知しただけでモジュールを持たないキャラ（`record_value_missing` 参照）。
    pub source: String,
    pub modules: Vec<Module>,
    /// 直近のキャプチャで属性値の取得に起きた異常（None=正常 or 未キャプチャ）。
    /// `Missing` のときは `modules` を更新しておらず、前回取れたデータのまま。
    pub value_issue: Option<bpsr_core::engine::modules::ValueIssue>,
}

#[derive(Default)]
pub struct Inner {
    /// 検知順（初回検知順）に並ぶキャラ一覧。
    pub chars: Vec<CharacterEntry>,
    /// 明示選択中のキャラ。None なら自動解決（`effective_index` 参照）。
    pub selected_char_id: Option<i64>,
    /// `BPSR_MODULE_DUMP` が設定され、char_id=0 の擬似キャラを検証用途として
    /// 明示的に固定表示するモード。起動時に一度だけ確定させる。
    pub env_fixed: bool,
}

pub type SharedState = Arc<Mutex<Inner>>;

pub fn new() -> SharedState {
    Arc::new(Mutex::new(Inner::default()))
}

impl Inner {
    /// 有効キャラのインデックス（優先順位はモジュールドキュメント参照）。
    /// キャラが1件も無ければ None。
    fn effective_index(&self) -> Option<usize> {
        if let Some(id) = self.selected_char_id {
            if let Some(idx) = self.chars.iter().position(|c| c.char_id == id) {
                return Some(idx);
            }
        }
        if self.env_fixed {
            if let Some(idx) = self.chars.iter().position(|c| c.char_id == 0) {
                return Some(idx);
            }
        }
        if let Some(idx) = self.chars.iter().position(|c| c.char_id != 0) {
            return Some(idx);
        }
        (!self.chars.is_empty()).then_some(0)
    }

    /// 有効キャラの参照。
    pub fn effective_char(&self) -> Option<&CharacterEntry> {
        self.effective_index().map(|i| &self.chars[i])
    }

    /// 有効キャラの char_id（自動採用時も含む）。
    pub fn effective_char_id(&self) -> Option<i64> {
        self.effective_char().map(|c| c.char_id)
    }

    /// char_id のエントリを取得、無ければ検知順末尾へ新規作成して返す。
    pub fn upsert_char(&mut self, char_id: i64) -> &mut CharacterEntry {
        if let Some(idx) = self.chars.iter().position(|c| c.char_id == char_id) {
            return &mut self.chars[idx];
        }
        self.chars.push(CharacterEntry {
            char_id,
            ..Default::default()
        });
        self.chars.last_mut().expect("直前に push した要素")
    }

    /// 属性値が全件取れなかったキャプチャ（`ValueIssue::Missing`）を記録する。
    /// モジュール一覧・最終更新時刻には触れず、警告と名前だけを残す（前回正しく取れた
    /// データを使い続けるため）。初検知のキャラはモジュール 0 件・取得元 "none" で作る。
    ///
    /// 有効キャラが変わったら true を返す。他に実キャラがいない状態（擬似キャラ char_id=0
    /// だけを見ている等）で初検知のキャラが現れると、優先順位により有効キャラがそちらへ
    /// 移るため、呼び出し側は結果のリセット通知（modules-updated, payload 0）を出してフロントの古い結果を
    /// 片付ける必要がある。
    /// `total` は今回のキャプチャで見つかったモジュール数（`ValueIssue::Missing` の件数）。
    /// `ValueIssue` ごと受け取らないのは、`Partial` を誤って渡せないようにするため。
    pub fn record_value_missing(&mut self, char_id: i64, name: &str, total: usize) -> bool {
        let before = self.effective_char_id();
        let entry = self.upsert_char(char_id);
        if entry.source.is_empty() {
            entry.source = "none".to_string();
        }
        entry.value_issue = Some(bpsr_core::engine::modules::ValueIssue::Missing { total });
        if !name.is_empty() {
            entry.name = name.to_string();
        }
        self.effective_char_id() != before
    }

    /// キャラ一覧の内容署名（char_id・名前・モジュール件数のみ。last_update_ms は
    /// 含めない＝時刻が進むだけの更新では変化しない）。characters.json の
    /// 書込抑止（内容が変わった時だけ書く）に使う。
    ///
    /// XOR だけの畳み込みは同一の (char_id, name, count) の組が異なる並びで
    /// 打ち消し合う（自己相殺）ため、`rotate_left` で順序依存にしている。
    pub fn characters_signature(&self) -> u64 {
        let mut sig = self.chars.len() as u64;
        for c in &self.chars {
            // FNV-1a によるキャラ名ハッシュ（低コストな内容署名で十分）。
            let mut name_hash: u64 = 0xcbf2_9ce4_8422_2325;
            for b in c.name.as_bytes() {
                name_hash ^= *b as u64;
                name_hash = name_hash.wrapping_mul(0x0000_0100_0000_01B3);
            }
            let entry_sig = (c.char_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ name_hash
                ^ (c.modules.len() as u64).wrapping_mul(0x0100_0000_01B3);
            sig = sig.rotate_left(7) ^ entry_sig;
        }
        sig
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_module() -> Module {
        Module {
            key: 0,
            uuid: 0,
            config_id: 0,
            name: String::new(),
            category: "attack".to_string(),
            quality: 0,
            parts: vec![],
        }
    }

    fn entry(char_id: i64, name: &str, mod_count: usize) -> CharacterEntry {
        entry_with_source(char_id, name, mod_count, "capture")
    }

    const MISSING: bpsr_core::engine::modules::ValueIssue =
        bpsr_core::engine::modules::ValueIssue::Missing { total: 10 };

    #[test]
    fn record_value_missing_keeps_existing_modules() {
        let mut s = Inner {
            chars: vec![entry(7, "七番", 3)],
            ..Default::default()
        };
        let changed = s.record_value_missing(7, "七番改", 10);
        assert!(!changed);
        let c = &s.chars[0];
        assert_eq!(c.modules.len(), 3, "前回のモジュールを消してはいけない");
        assert_eq!(c.last_update_ms, Some(1), "最終更新時刻は進めない");
        assert_eq!(c.source, "capture");
        assert_eq!(c.name, "七番改");
        assert_eq!(c.value_issue, Some(MISSING));
    }

    #[test]
    fn record_value_missing_reports_effective_switch_from_pseudo_char() {
        // 擬似キャラ（char_id=0）だけを見ている状態で初検知の実キャラが現れると、
        // 優先順位で有効キャラがそちらへ移る。呼び出し側が結果のリセットを通知できるよう true。
        let mut s = Inner {
            chars: vec![entry_with_source(0, "", 5, "dump")],
            ..Default::default()
        };
        let changed = s.record_value_missing(9, "", 10);
        assert!(changed);
        assert_eq!(s.effective_char_id(), Some(9));
        let c = s.effective_char().expect("有効キャラ");
        assert!(c.modules.is_empty());
        assert_eq!(c.source, "none", "StatusDto.source の契約（capture|dump|none）を守る");
        assert_eq!(c.value_issue, Some(MISSING));
    }

    fn entry_with_source(
        char_id: i64,
        name: &str,
        mod_count: usize,
        source: &str,
    ) -> CharacterEntry {
        CharacterEntry {
            char_id,
            name: name.to_string(),
            last_update_ms: Some(1),
            source: source.to_string(),
            modules: (0..mod_count).map(|_| test_module()).collect(),
            value_issue: None,
        }
    }

    #[test]
    fn effective_char_is_none_when_no_chars() {
        let inner = Inner::default();
        assert!(inner.effective_char().is_none());
    }

    #[test]
    fn effective_char_falls_back_to_first_when_unselected() {
        let mut inner = Inner::default();
        inner.chars.push(entry(1, "A", 1));
        inner.chars.push(entry(2, "B", 2));
        assert_eq!(inner.effective_char_id(), Some(1));
    }

    #[test]
    fn effective_char_honors_explicit_selection() {
        let mut inner = Inner::default();
        inner.chars.push(entry(1, "A", 1));
        inner.chars.push(entry(2, "B", 2));
        inner.selected_char_id = Some(2);
        assert_eq!(inner.effective_char_id(), Some(2));
    }

    #[test]
    fn effective_char_ignores_unknown_selection_and_falls_back() {
        let mut inner = Inner::default();
        inner.chars.push(entry(1, "A", 1));
        inner.selected_char_id = Some(999);
        assert_eq!(inner.effective_char_id(), Some(1));
    }

    #[test]
    fn upsert_char_reuses_existing_entry_and_preserves_detection_order() {
        let mut inner = Inner::default();
        inner.upsert_char(1).name = "A".to_string();
        inner.upsert_char(2).name = "B".to_string();
        inner.upsert_char(1).name = "A改".to_string();
        assert_eq!(inner.chars.len(), 2);
        assert_eq!(inner.chars[0].char_id, 1);
        assert_eq!(inner.chars[0].name, "A改");
        assert_eq!(inner.chars[1].char_id, 2);
    }

    #[test]
    fn characters_signature_ignores_last_update_ms_but_detects_name_and_count_changes() {
        let mut inner = Inner::default();
        inner.chars.push(entry(1, "A", 1));
        let sig1 = inner.characters_signature();

        inner.chars[0].last_update_ms = Some(999_999);
        assert_eq!(
            sig1,
            inner.characters_signature(),
            "last_update_ms のみの変化では署名は変わらない"
        );

        inner.chars[0].name = "B".to_string();
        assert_ne!(
            sig1,
            inner.characters_signature(),
            "名前変化は署名に反映される"
        );

        inner.chars[0].name = "A".to_string();
        inner.chars[0].modules.push(test_module());
        assert_ne!(
            sig1,
            inner.characters_signature(),
            "モジュール件数変化は署名に反映される"
        );
    }

    #[test]
    fn characters_signature_is_order_dependent() {
        // XOR だけの畳み込みは同じ要素集合の順序違いを区別できない（自己相殺）。
        // rotate_left による順序依存化で、並び順が違えば署名も変わることを固定する。
        let mut a = Inner::default();
        a.chars.push(entry(1, "A", 1));
        a.chars.push(entry(2, "B", 2));

        let mut b = Inner::default();
        b.chars.push(entry(2, "B", 2));
        b.chars.push(entry(1, "A", 1));

        assert_ne!(a.characters_signature(), b.characters_signature());
    }

    #[test]
    fn effective_char_prefers_env_fixed_pseudo_char_over_capture() {
        // BPSR_MODULE_DUMP 指定時（env_fixed）は、実キャラをライブ検知していても
        // 明示選択が無い限り char_id=0 の擬似キャラを有効キャラとする（検証用途の
        // 明示的意図を尊重する）。
        let mut inner = Inner {
            env_fixed: true,
            ..Default::default()
        };
        inner.chars.push(entry_with_source(0, "", 5, "dump"));
        inner
            .chars
            .push(entry_with_source(123, "実キャラ", 10, "capture"));
        assert_eq!(inner.effective_char_id(), Some(0));
    }

    #[test]
    fn effective_char_prefers_live_capture_over_legacy_pseudo_char_without_env_fixed() {
        // レガシー owned_modules.json（char_id=0 の擬似キャラ）だけが存在する状態から
        // 実キャラをライブ検知したら、明示選択が無い限り実キャラが有効になる
        // （擬似キャラが先着として居座って実キャラの更新を隠す回帰の防止）。
        let mut inner = Inner::default();
        inner.chars.push(entry_with_source(0, "", 5, "dump"));
        inner
            .chars
            .push(entry_with_source(123, "実キャラ", 10, "capture"));
        assert_eq!(inner.effective_char_id(), Some(123));
    }

    #[test]
    fn effective_char_prefers_real_char_over_pseudo_even_when_both_dump_sourced() {
        // 3段目の判定は char_id != 0 のみで、可変の source は見ない。
        // すべて dump 由来（capture 由来が1件も無い）でも、擬似キャラ(char_id=0)より
        // 実キャラを優先する。source をゾーン移動や reload_from_dump で書き換えても
        // 有効キャラの解決結果が変わらない（実行中に安定している）ことを固定する
        // （旧ルールでは capture 由来が無いと擬似キャラへフォールバックしてしまい、
        // レガシーお気に入りが恒久的に迷子になる副作用があった）。
        let mut inner = Inner::default();
        inner.chars.push(entry_with_source(0, "", 5, "dump"));
        inner.chars.push(entry_with_source(7, "B", 3, "dump"));
        assert_eq!(inner.effective_char_id(), Some(7));
    }

    #[test]
    fn effective_char_falls_back_to_pseudo_char_when_it_is_the_only_one() {
        // 擬似キャラしか検知していない（実キャラ未検知）場合は、それがそのまま有効。
        let mut inner = Inner::default();
        inner.chars.push(entry_with_source(0, "", 5, "dump"));
        assert_eq!(inner.effective_char_id(), Some(0));
    }

    #[test]
    fn explicit_selection_overrides_env_fixed_and_capture_priority() {
        let mut inner = Inner {
            env_fixed: true,
            selected_char_id: Some(123),
            ..Default::default()
        };
        inner.chars.push(entry_with_source(0, "", 5, "dump"));
        inner
            .chars
            .push(entry_with_source(123, "実キャラ", 10, "capture"));
        assert_eq!(inner.effective_char_id(), Some(123));
    }
}
