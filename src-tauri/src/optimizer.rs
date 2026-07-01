//! モジュール最適化: 所持モジュールから slot_count 枠の組み合わせを全探索し、
//! 「レベル分布 → リンク効果」の辞書式優先度で最良を求める。
//!
//! 評価指標（ゲーム内モジュール画面の「パワーコア効果」「リンク効果」に対応）:
//! - 各属性は slot_count 枠分の値を合計し、閾値 [1,4,8,12,16,20] でレベル(0〜6)化。
//!   閾値はスロット数に依存しない（合計値20でLv6）。
//! - リンク効果 = 全属性値の合計（画面右上の数値）。
//!
//! ランキング（辞書式・すべて最大化）:
//!   1. 選択属性が Lv6 到達した数（選択を優先）
//!   2. Lv6 属性の総数
//!   3. Lv5 属性の総数
//!   4. 全属性レベルの合計（余りの属性も高く）
//!   5. リンク効果（全属性値の合計）= 表示スコア

use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// 属性レベルの閾値（値 >= 閾値 でその段に到達）。
const ATTR_THRESHOLDS: [i32; 6] = [1, 4, 8, 12, 16, 20];

#[inline]
fn level_of(v: i32) -> usize {
    ATTR_THRESHOLDS.iter().take_while(|&&t| v >= t).count()
}

/// `combo`（昇順の候補インデックス列）を辞書式に次の組み合わせへ進める。
/// これ以上進めない（最後の組み合わせだった）場合は false を返す。
/// n 個から combo.len() 個を選ぶ全組み合わせを、任意のスロット数で列挙するために使う。
fn next_combination(combo: &mut [usize], n: usize) -> bool {
    let k = combo.len();
    let mut i = k;
    while i > 0 {
        i -= 1;
        // i 番目を進められる（右側の要素分の余地がある）か。
        if combo[i] != i + n - k {
            combo[i] += 1;
            for j in (i + 1)..k {
                combo[j] = combo[j - 1] + 1;
            }
            return true;
        }
    }
    false
}

// ---- DTO ----

#[derive(Debug, Clone, Serialize)]
pub struct Part {
    pub attr_id: i32,
    pub attr_name: String,
    pub value: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Module {
    /// items マップのキー（mod_infos との突合キー）。モジュールの一意識別に使う。
    pub key: i64,
    pub uuid: i64,
    pub config_id: i32,
    pub name: String,
    /// "attack" | "guardian" | "support" | "unknown"
    pub category: String,
    pub quality: i32,
    pub parts: Vec<Part>,
}

impl Module {
    pub fn from_core(m: &bpsr_core::engine::modules::ModuleInfo) -> Self {
        Self {
            key: m.key,
            uuid: m.uuid,
            config_id: m.config_id,
            // 名称はゲーム公式の日本語名へ解決（未知は core の英語名にフォールバック）。
            name: crate::attrs::module_name(m.config_id)
                .map(str::to_string)
                .unwrap_or_else(|| m.name.to_string()),
            category: category_of(m.config_id).to_string(),
            quality: m.quality,
            parts: m
                .parts
                .iter()
                .map(|p| Part {
                    attr_id: p.attr_id,
                    attr_name: crate::attrs::attr_name(p.attr_id)
                        .map(str::to_string)
                        .unwrap_or_else(|| p.attr_name.to_string()),
                    value: p.value,
                })
                .collect(),
        }
    }
}

/// config_id からモジュールカテゴリを判定（55001xx=攻撃 / 55002xx=辅助 / 55003xx=守護）。
pub fn category_of(config_id: i32) -> &'static str {
    match config_id / 100 % 10 {
        1 => "attack",
        2 => "support",
        3 => "guardian",
        _ => "unknown",
    }
}

/// 属性ごとの値・レベル内訳（UI 表示用）。
#[derive(Debug, Clone, Serialize)]
pub struct AttrBreakdown {
    pub attr_id: i32,
    pub attr_name: String,
    pub value: i32,
    pub level: usize,
    /// ユーザーが選択した目標属性か。
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Solution {
    pub modules: Vec<Module>,
    /// リンク効果（全属性値の合計）= 表示スコア。
    pub link_effect: i32,
    pub lv6_count: usize,
    pub lv5_count: usize,
    /// 選択属性のうち Lv6 に到達した数。
    pub selected_lv6: usize,
    /// 全属性レベルの合計。
    pub level_sum: usize,
    /// 全属性の内訳（レベル降順）。
    pub breakdown: Vec<AttrBreakdown>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OptimizeResult {
    pub solutions: Vec<Solution>,
    pub candidate_count: usize,
    pub combinations: u64,
}

/// 辞書式比較キー（すべて最大化）。
type Key = (usize, usize, usize, usize, i32);

/// 最適化本体。
/// - `slot_count`: 装備枠数（この個数の組み合わせを全探索する）。
/// - `category`: Some なら該当カテゴリのモジュールのみを枠に使う。
/// - `exclude_ids`: いずれかを含むモジュールは候補から除外。
/// - `require_level`: Some(lv) なら、全選択属性が lv 以上に到達する組み合わせのみ採用。
pub fn optimize(
    modules: &[Module],
    selected_ids: &[i32],
    category: Option<&str>,
    exclude_ids: &[i32],
    requirements: &[(i32, usize)],
    top_k: usize,
    slot_count: usize,
) -> OptimizeResult {
    // カテゴリ絞り込み・除外属性を含むモジュールを除いた候補。
    let candidates: Vec<&Module> = modules
        .iter()
        .filter(|m| category.is_none_or(|c| c == "all" || m.category == c))
        .filter(|m| !m.parts.iter().any(|p| exclude_ids.contains(&p.attr_id)))
        .collect();

    let n = candidates.len();
    if slot_count == 0 || n < slot_count {
        return OptimizeResult {
            solutions: Vec::new(),
            candidate_count: n,
            combinations: 0,
        };
    }

    // 属性IDを密なインデックスに割り当て（高速集計用）。
    let mut id_to_idx: HashMap<i32, usize> = HashMap::new();
    let mut idx_to_id: Vec<i32> = Vec::new();
    for m in &candidates {
        for p in &m.parts {
            id_to_idx.entry(p.attr_id).or_insert_with(|| {
                idx_to_id.push(p.attr_id);
                idx_to_id.len() - 1
            });
        }
    }
    let n_attr = idx_to_id.len();
    let selected_mask: Vec<bool> = idx_to_id.iter().map(|id| selected_ids.contains(id)).collect();

    // 属性ごとの下限レベル要求を idx へ解決。候補に存在しない属性が必須なら達成不能。
    let mut required_idxs: Vec<(usize, usize)> = Vec::new();
    for &(attr_id, lv) in requirements {
        if lv == 0 {
            continue;
        }
        match id_to_idx.get(&attr_id) {
            Some(&idx) => required_idxs.push((idx, lv)),
            None => {
                return OptimizeResult {
                    solutions: Vec::new(),
                    candidate_count: n,
                    combinations: 0,
                };
            }
        }
    }

    // 各候補を (idx, value) ペア列に変換。
    let vecs: Vec<Vec<(usize, i32)>> = candidates
        .iter()
        .map(|m| {
            let mut v: Vec<(usize, i32)> = Vec::with_capacity(m.parts.len());
            for p in &m.parts {
                v.push((id_to_idx[&p.attr_id], p.value));
            }
            v
        })
        .collect();

    let mut totals = vec![0i32; n_attr];
    let mut touched: Vec<usize> = Vec::with_capacity(12);
    let mut heap: BinaryHeap<Reverse<(Key, Vec<usize>)>> = BinaryHeap::new();
    let mut combinations: u64 = 0;

    // n 個から slot_count 個を選ぶ組み合わせを辞書式に全列挙。
    let mut combo: Vec<usize> = (0..slot_count).collect();
    loop {
        combinations += 1;

        // 集計（触れた idx のみ）。
        for &combo_idx in &combo {
            for &(idx, val) in &vecs[combo_idx] {
                if totals[idx] == 0 {
                    touched.push(idx);
                }
                totals[idx] += val;
            }
        }

        // キー算出。
        let mut lv6 = 0usize;
        let mut lv5 = 0usize;
        let mut sel_lv6 = 0usize;
        let mut level_sum = 0usize;
        let mut link = 0i32;
        for &idx in &touched {
            let v = totals[idx];
            let lv = level_of(v);
            level_sum += lv;
            link += v;
            if lv == 6 {
                lv6 += 1;
            } else if lv == 5 {
                lv5 += 1;
            }
            if selected_mask[idx] && lv == 6 {
                sel_lv6 += 1;
            }
        }

        // 属性ごとの下限Lv要求（totals がまだ有効なうちに判定）。
        let req_ok = required_idxs
            .iter()
            .all(|&(idx, lv)| level_of(totals[idx]) >= lv);

        // touched をリセット。
        for &idx in &touched {
            totals[idx] = 0;
        }
        touched.clear();

        if req_ok {
            let key: Key = (sel_lv6, lv6, lv5, level_sum, link);
            if heap.len() < top_k {
                heap.push(Reverse((key, combo.clone())));
            } else if let Some(Reverse((min_key, _))) = heap.peek() {
                if key > *min_key {
                    heap.pop();
                    heap.push(Reverse((key, combo.clone())));
                }
            }
        }

        if !next_combination(&mut combo, n) {
            break;
        }
    }

    // キー降順に並べる。
    let mut ranked: Vec<(Key, Vec<usize>)> = heap.into_iter().map(|Reverse(x)| x).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0));

    let solutions = ranked
        .into_iter()
        .map(|(_, idxs)| {
            let mods: Vec<Module> = idxs.iter().map(|&i| candidates[i].clone()).collect();
            build_solution(mods, selected_ids)
        })
        .collect();

    OptimizeResult {
        solutions,
        candidate_count: n,
        combinations,
    }
}

/// 選択された各モジュールから内訳と各指標を再構成して Solution を作る。
fn build_solution(mods: Vec<Module>, selected_ids: &[i32]) -> Solution {
    let mut totals: HashMap<i32, (String, i32)> = HashMap::new();
    for m in &mods {
        for p in &m.parts {
            let e = totals.entry(p.attr_id).or_insert((p.attr_name.clone(), 0));
            e.1 += p.value;
        }
    }

    let mut breakdown: Vec<AttrBreakdown> = totals
        .into_iter()
        .map(|(attr_id, (attr_name, value))| AttrBreakdown {
            attr_id,
            attr_name,
            value,
            level: level_of(value),
            selected: selected_ids.contains(&attr_id),
        })
        .collect();
    // レベル降順 → 値降順 → 選択優先。
    breakdown.sort_by(|a, b| {
        b.level
            .cmp(&a.level)
            .then(b.value.cmp(&a.value))
            .then(b.selected.cmp(&a.selected))
    });

    let lv6_count = breakdown.iter().filter(|b| b.level == 6).count();
    let lv5_count = breakdown.iter().filter(|b| b.level == 5).count();
    let selected_lv6 = breakdown
        .iter()
        .filter(|b| b.selected && b.level == 6)
        .count();
    let level_sum = breakdown.iter().map(|b| b.level).sum();
    let link_effect = breakdown.iter().map(|b| b.value).sum();

    Solution {
        modules: mods,
        link_effect,
        lv6_count,
        lv5_count,
        selected_lv6,
        level_sum,
        breakdown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(key: i64, config_id: i32, parts: &[(i32, i32)]) -> Module {
        Module {
            key,
            uuid: key,
            config_id,
            name: "Test".into(),
            category: category_of(config_id).to_string(),
            quality: 4,
            parts: parts
                .iter()
                .map(|&(attr_id, value)| Part {
                    attr_id,
                    attr_name: format!("attr{attr_id}"),
                    value,
                })
                .collect(),
        }
    }

    #[test]
    fn level_thresholds() {
        assert_eq!(level_of(0), 0);
        assert_eq!(level_of(3), 1);
        assert_eq!(level_of(8), 3);
        assert_eq!(level_of(20), 6);
        assert_eq!(level_of(21), 6);
    }

    #[test]
    fn prefers_more_lv6() {
        // 2枠で attr1=20(Lv6), 別案は attr1=16(Lv5)+attr2=16(Lv5)。
        // Lv6 数が多い前者を優先するはず。
        let modules = vec![
            module(1, 5500103, &[(1, 10)]),
            module(2, 5500103, &[(1, 10)]),
            module(3, 5500103, &[(1, 8), (2, 8)]),
            module(4, 5500103, &[(1, 8), (2, 8)]),
        ];
        // 全4枠使用: attr1=10+10+8+8=36(Lv6), attr2=8+8=16(Lv5) → lv6=1,lv5=1
        let res = optimize(&modules, &[], None, &[], &[], 5, 4);
        let best = &res.solutions[0];
        assert_eq!(best.lv6_count, 1);
        assert_eq!(best.lv5_count, 1);
        assert_eq!(best.link_effect, 52);
    }

    #[test]
    fn selected_lv6_takes_priority() {
        // 選択 attr=9。2案: (A) 選択を Lv6(20) にする, (B) 非選択を Lv6 にし選択は低い。
        let modules = vec![
            module(1, 5500103, &[(9, 10)]),
            module(2, 5500103, &[(9, 10)]),
            module(3, 5500103, &[(9, 1), (5, 20)]),
            module(4, 5500103, &[(9, 1), (5, 20)]),
        ];
        // 全枠: attr9=22(Lv6), attr5=40(Lv6) → 両方Lv6だが選択(9)もLv6
        let res = optimize(&modules, &[9], None, &[], &[], 5, 4);
        let best = &res.solutions[0];
        assert!(best.selected_lv6 >= 1);
    }

    #[test]
    fn require_level_filters() {
        // 選択 attr=9 に Lv6 必須。到達不能なら解なし。
        let modules = vec![
            module(1, 5500103, &[(9, 3)]),
            module(2, 5500103, &[(9, 3)]),
            module(3, 5500103, &[(9, 3)]),
            module(4, 5500103, &[(9, 3)]),
        ];
        // 合計 12 → Lv4 のみ。attr9 を Lv6必須なら解なし。
        let res = optimize(&modules, &[9], None, &[], &[(9, 6)], 5, 4);
        assert!(res.solutions.is_empty());
        // Lv4必須なら成立。
        let res2 = optimize(&modules, &[9], None, &[], &[(9, 4)], 5, 4);
        assert_eq!(res2.solutions.len(), 1);
    }

    #[test]
    fn five_slots_selects_five_modules() {
        // 5枠指定時は5モジュールを選び、合計もその5枠分になること。
        let modules = vec![
            module(1, 5500103, &[(1, 5)]),
            module(2, 5500103, &[(1, 5)]),
            module(3, 5500103, &[(1, 5)]),
            module(4, 5500103, &[(1, 5)]),
            module(5, 5500103, &[(1, 5)]),
            module(6, 5500103, &[(1, 1)]),
        ];
        // 上位5モジュール(各5) → attr1=25(Lv6), link=25。6番目(1)は不採用。
        let res = optimize(&modules, &[1], None, &[], &[], 5, 5);
        let best = &res.solutions[0];
        assert_eq!(best.modules.len(), 5);
        assert_eq!(best.link_effect, 25);
        assert_eq!(best.lv6_count, 1);
        // C(6,5)=6 通り。
        assert_eq!(res.combinations, 6);
    }

    #[test]
    fn too_few_candidates_for_slots() {
        // 候補が slot_count 未満なら解なし。
        let modules = vec![
            module(1, 5500103, &[(1, 5)]),
            module(2, 5500103, &[(1, 5)]),
            module(3, 5500103, &[(1, 5)]),
            module(4, 5500103, &[(1, 5)]),
        ];
        let res = optimize(&modules, &[1], None, &[], &[], 5, 5);
        assert!(res.solutions.is_empty());
        assert_eq!(res.candidate_count, 4);
    }
}
