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
//!
//! 探索方式: C(n, slot_count) の全列挙だが、辞書式に枠を深さ優先で選び、ランキングキーを
//! [`Accum`] で差分更新する（共有プレフィックスの再集計を避ける）。最初の枠を rayon で
//! 並列化し、各タスクが保持したローカル top-k の和集合から全体 top-k を再構成する。
//! 同点キーは combo 昇順で決定的に順序付けるため、スレッド数に依らず結果は再現可能。

use rayon::prelude::*;
use serde::Serialize;
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};

/// 属性レベルの閾値（値 >= 閾値 でその段に到達）。
const ATTR_THRESHOLDS: [i32; 6] = [1, 4, 8, 12, 16, 20];

#[inline]
fn level_of(v: i32) -> usize {
    ATTR_THRESHOLDS.iter().take_while(|&&t| v >= t).count()
}

/// n 個から k 個を選ぶ組み合わせ数 C(n, k)。中間計算を u128 で行い桁溢れを避ける。
fn n_choose_k(n: usize, k: usize) -> u64 {
    if k > n {
        return 0;
    }
    let k = k.min(n - k);
    let mut res: u128 = 1;
    for i in 0..k as u128 {
        // 各段で res == C(n, i+1) となり常に整数（i+1 個の連続整数の積は (i+1)! で割り切れる）。
        res = res * (n as u128 - i) / (i + 1);
    }
    res as u64
}

/// 探索候補1件の寄与。属性は密インデックス化済み。
struct Cand {
    /// (属性の密インデックス, 値)。
    parts: Vec<(u32, i32)>,
}

/// DFS で差分更新するランキング集計。add/remove は「触れた属性数」に比例した O(1) 相当で
/// キー要素（選択Lv6数・Lv6数・Lv5数・レベル合計・リンク効果）を保つ。
/// 属性値は正なので add でレベルは単調増加し、level_sum の usize 減算は起きない。
struct Accum {
    totals: Vec<i32>,
    level_sum: usize,
    lv6: usize,
    lv5: usize,
    sel_lv6: usize,
    link: i32,
}

impl Accum {
    fn new(n_attr: usize) -> Self {
        Self {
            totals: vec![0; n_attr],
            level_sum: 0,
            lv6: 0,
            lv5: 0,
            sel_lv6: 0,
            link: 0,
        }
    }

    #[inline]
    fn add(&mut self, cand: &Cand, selected_mask: &[bool]) {
        for &(idx, val) in &cand.parts {
            let idx = idx as usize;
            let old = self.totals[idx];
            let new = old + val;
            let old_lv = level_of(old);
            let new_lv = level_of(new);
            if new_lv != old_lv {
                self.level_sum += new_lv - old_lv;
                match old_lv {
                    6 => self.lv6 -= 1,
                    5 => self.lv5 -= 1,
                    _ => {}
                }
                match new_lv {
                    6 => self.lv6 += 1,
                    5 => self.lv5 += 1,
                    _ => {}
                }
                if selected_mask[idx] {
                    if old_lv == 6 {
                        self.sel_lv6 -= 1;
                    }
                    if new_lv == 6 {
                        self.sel_lv6 += 1;
                    }
                }
            }
            self.totals[idx] = new;
            self.link += val;
        }
    }

    #[inline]
    fn remove(&mut self, cand: &Cand, selected_mask: &[bool]) {
        for &(idx, val) in &cand.parts {
            let idx = idx as usize;
            let cur = self.totals[idx];
            let new = cur - val;
            let cur_lv = level_of(cur);
            let new_lv = level_of(new);
            if cur_lv != new_lv {
                self.level_sum -= cur_lv - new_lv;
                match cur_lv {
                    6 => self.lv6 -= 1,
                    5 => self.lv5 -= 1,
                    _ => {}
                }
                match new_lv {
                    6 => self.lv6 += 1,
                    5 => self.lv5 += 1,
                    _ => {}
                }
                if selected_mask[idx] {
                    if cur_lv == 6 {
                        self.sel_lv6 -= 1;
                    }
                    if new_lv == 6 {
                        self.sel_lv6 += 1;
                    }
                }
            }
            self.totals[idx] = new;
            self.link -= val;
        }
    }

    #[inline]
    fn key(&self) -> Key {
        (self.sel_lv6, self.lv6, self.lv5, self.level_sum, self.link)
    }
}

/// top-k 保持用の要素。全順序 = キー昇順 → combo 降順（＝ goodness: キー降順・combo 昇順）。
/// これにより同点キーは辞書式に小さい combo を優先し、スレッド数に依らず結果が決定的になる。
struct Ranked {
    key: Key,
    combo: Vec<u32>,
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.combo == other.combo
    }
}
impl Eq for Ranked {}
impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key
            .cmp(&other.key)
            .then_with(|| other.combo.cmp(&self.combo))
    }
}
impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 上位 k 件を保持する最小ヒープ（root = 保持中で最も劣る要素）。
struct TopK {
    heap: BinaryHeap<Reverse<Ranked>>,
    cap: usize,
}

impl TopK {
    fn new(cap: usize) -> Self {
        Self {
            heap: BinaryHeap::new(),
            cap,
        }
    }

    /// 現在の combo（path）とキーを候補として投入する。combo の確保は採用時のみ。
    #[inline]
    fn offer(&mut self, key: Key, path: &[u32]) {
        if self.heap.len() < self.cap {
            self.heap.push(Reverse(Ranked {
                key,
                combo: path.to_vec(),
            }));
            return;
        }
        let worst = &self.heap.peek().expect("cap>=1 なら非空").0;
        let better = match key.cmp(&worst.key) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => path < worst.combo.as_slice(),
        };
        if better {
            self.heap.pop();
            self.heap.push(Reverse(Ranked {
                key,
                combo: path.to_vec(),
            }));
        }
    }

    fn into_vec(self) -> Vec<Ranked> {
        self.heap.into_iter().map(|Reverse(r)| r).collect()
    }
}

/// 残り (slot_count - depth) 枠を index `start` 以降から選ぶ再帰列挙。
/// acc は呼び出し前後で不変（各候補を add→再帰→remove）。leaf で下限Lv要求を判定し top-k へ。
#[allow(clippy::too_many_arguments)]
fn dfs(
    depth: usize,
    start: usize,
    slot_count: usize,
    n: usize,
    cands: &[Cand],
    selected_mask: &[bool],
    required_idxs: &[(usize, usize)],
    acc: &mut Accum,
    path: &mut [u32],
    top: &mut TopK,
) {
    if depth == slot_count {
        if required_idxs
            .iter()
            .all(|&(idx, lv)| level_of(acc.totals[idx]) >= lv)
        {
            top.offer(acc.key(), path);
        }
        return;
    }
    // この深さで選べる最大インデックス（右側に残り枠分の余地を残す）。
    let last = n - (slot_count - depth);
    for i in start..=last {
        acc.add(&cands[i], selected_mask);
        path[depth] = i as u32;
        dfs(
            depth + 1,
            i + 1,
            slot_count,
            n,
            cands,
            selected_mask,
            required_idxs,
            acc,
            path,
            top,
        );
        acc.remove(&cands[i], selected_mask);
    }
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

    // 各候補を密インデックスの寄与列へ変換。
    let cands: Vec<Cand> = candidates
        .iter()
        .map(|m| Cand {
            parts: m
                .parts
                .iter()
                .map(|p| (id_to_idx[&p.attr_id] as u32, p.value))
                .collect(),
        })
        .collect();

    // 組み合わせ総数は直接算出（下限Lv要求で除外される分も含む＝旧実装と同義）。
    let combinations = n_choose_k(n, slot_count);

    // 最初の枠を並列化。各タスクは部分木を DFS し、ローカル top-k を返す。
    // 部分木ごとに保持した top-k の和集合は全体 top-k を必ず包含するため、
    // それらを goodness 降順で並べ直して上位 top_k を採れば逐次実装と同じ結果になる。
    let last0 = n - slot_count; // 最初の枠が取り得る最大インデックス。
    let mut ranked: Vec<Ranked> = (0..last0 + 1)
        .into_par_iter()
        .map(|i0| {
            let mut acc = Accum::new(n_attr);
            let mut path = vec![0u32; slot_count];
            let mut top = TopK::new(top_k);
            acc.add(&cands[i0], &selected_mask);
            path[0] = i0 as u32;
            dfs(
                1,
                i0 + 1,
                slot_count,
                n,
                &cands,
                &selected_mask,
                &required_idxs,
                &mut acc,
                &mut path,
                &mut top,
            );
            top.into_vec()
        })
        .reduce(Vec::new, |mut a, mut b| {
            a.append(&mut b);
            a
        });

    // goodness 降順（キー降順 → combo 昇順）に並べて上位を採る。
    ranked.sort_by(|a, b| b.cmp(a));
    ranked.truncate(top_k);

    let solutions = ranked
        .into_iter()
        .map(|r| {
            let mods: Vec<Module> = r
                .combo
                .iter()
                .map(|&i| candidates[i as usize].clone())
                .collect();
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

    /// 実データ(../../extracted_game_data/owned_modules.json)で探索時間を計測する。
    /// 実行: `cargo test --release -p bpsr-module-optimizer bench_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_real_data() {
        use std::time::Instant;

        #[derive(serde::Deserialize)]
        struct P {
            attr_id: i32,
            #[serde(default)]
            attr_name: String,
            value: i32,
        }
        #[derive(serde::Deserialize)]
        struct M {
            key: i64,
            #[serde(default)]
            uuid: i64,
            config_id: i32,
            #[serde(default)]
            name: String,
            #[serde(default)]
            quality: i32,
            parts: Vec<P>,
        }

        let path = std::env::var("BPSR_MODULE_DUMP")
            .unwrap_or_else(|_| "../../extracted_game_data/owned_modules.json".to_string());
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("読込失敗 {path}: {e}"));
        let raw: Vec<M> = serde_json::from_str(&text).expect("JSON 解析失敗");
        let modules: Vec<Module> = raw
            .into_iter()
            .map(|m| Module {
                key: m.key,
                uuid: m.uuid,
                config_id: m.config_id,
                name: m.name,
                category: category_of(m.config_id).to_string(),
                quality: m.quality,
                parts: m
                    .parts
                    .into_iter()
                    .map(|p| Part {
                        attr_id: p.attr_id,
                        attr_name: p.attr_name,
                        value: p.value,
                    })
                    .collect(),
            })
            .collect();
        eprintln!("modules: {}", modules.len());

        for &slot in &[4usize, 5usize] {
            // ウォームアップ1回 + 計測1回。
            let _ = optimize(&modules, &[2104], Some("all"), &[], &[], 5, slot);
            let t = Instant::now();
            let res = optimize(&modules, &[2104], Some("all"), &[], &[], 5, slot);
            let dt = t.elapsed();
            eprintln!(
                "slot={slot} all: combos={} cand={} best_link={} elapsed={:?}",
                res.combinations,
                res.candidate_count,
                res.solutions.first().map(|s| s.link_effect).unwrap_or(0),
                dt
            );
        }
    }

    /// 参照実装: 全組み合わせをゼロから集計してキーを求め、goodness 降順で top_k キーを返す。
    /// （最適化版と同じ辞書式・combo 昇順タイブレークで比較できるよう combo も返す）
    fn naive_ranked(
        modules: &[Module],
        selected_ids: &[i32],
        slot_count: usize,
        top_k: usize,
    ) -> Vec<(Key, Vec<usize>)> {
        let n = modules.len();
        let mut out: Vec<(Key, Vec<usize>)> = Vec::new();
        let mut combo: Vec<usize> = (0..slot_count).collect();
        loop {
            let mut totals: HashMap<i32, i32> = HashMap::new();
            for &c in &combo {
                for p in &modules[c].parts {
                    *totals.entry(p.attr_id).or_insert(0) += p.value;
                }
            }
            let (mut lv6, mut lv5, mut sel_lv6, mut level_sum, mut link) = (0, 0, 0, 0usize, 0i32);
            for (&id, &v) in &totals {
                let lv = level_of(v);
                level_sum += lv;
                link += v;
                if lv == 6 {
                    lv6 += 1;
                } else if lv == 5 {
                    lv5 += 1;
                }
                if selected_ids.contains(&id) && lv == 6 {
                    sel_lv6 += 1;
                }
            }
            out.push(((sel_lv6, lv6, lv5, level_sum, link), combo.clone()));
            // 次の組み合わせ（辞書式）。
            let k = slot_count;
            let mut i = k;
            let mut advanced = false;
            while i > 0 {
                i -= 1;
                if combo[i] != i + n - k {
                    combo[i] += 1;
                    for j in (i + 1)..k {
                        combo[j] = combo[j - 1] + 1;
                    }
                    advanced = true;
                    break;
                }
            }
            if !advanced {
                break;
            }
        }
        // goodness 降順: キー降順 → combo 昇順。
        out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        out.truncate(top_k);
        out
    }

    /// 決定的な合成データ（属性15種・値1〜9・各3〜4パーツ）。
    fn synth_modules(count: usize) -> Vec<Module> {
        let mut s: u64 = 0x9E3779B97F4A7C15;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        (0..count)
            .map(|k| {
                let n_parts = 3 + (next() % 2) as usize; // 3 or 4
                let parts = (0..n_parts)
                    .map(|_| {
                        let attr_id = 1000 + (next() % 15) as i32;
                        let value = 1 + (next() % 9) as i32;
                        Part {
                            attr_id,
                            attr_name: format!("a{attr_id}"),
                            value,
                        }
                    })
                    .collect();
                module_with_parts(k as i64, 5500103, parts)
            })
            .collect()
    }

    fn module_with_parts(key: i64, config_id: i32, parts: Vec<Part>) -> Module {
        Module {
            key,
            uuid: key,
            config_id,
            name: "S".into(),
            category: category_of(config_id).to_string(),
            quality: 4,
            parts,
        }
    }

    /// 最適化版が参照実装と完全一致（キー列・選択モジュール集合とも）することを検証。
    #[test]
    fn matches_naive_reference() {
        let modules = synth_modules(28);
        let selected = [1003, 1007, 1011];
        for &slot in &[4usize, 5usize] {
            let top_k = 12;
            let res = optimize(&modules, &selected, None, &[], &[], top_k, slot);
            let reference = naive_ranked(&modules, &selected, slot, top_k);

            assert_eq!(res.solutions.len(), reference.len(), "解の件数 slot={slot}");
            assert_eq!(
                res.combinations,
                n_choose_k(modules.len(), slot),
                "combinations は C(n,k) と一致すべき slot={slot}"
            );

            for (i, (sol, (key, combo))) in res.solutions.iter().zip(reference.iter()).enumerate() {
                // キー各要素の一致。
                assert_eq!(sol.selected_lv6, key.0, "sel_lv6 mismatch slot={slot} rank={i}");
                assert_eq!(sol.lv6_count, key.1, "lv6 mismatch slot={slot} rank={i}");
                assert_eq!(sol.lv5_count, key.2, "lv5 mismatch slot={slot} rank={i}");
                assert_eq!(sol.level_sum, key.3, "level_sum mismatch slot={slot} rank={i}");
                assert_eq!(sol.link_effect, key.4, "link mismatch slot={slot} rank={i}");
                // 選択されたモジュール集合（key=モジュールキー）も一致。
                let got: std::collections::BTreeSet<i64> =
                    sol.modules.iter().map(|m| m.key).collect();
                let want: std::collections::BTreeSet<i64> =
                    combo.iter().map(|&c| modules[c].key).collect();
                assert_eq!(got, want, "module set mismatch slot={slot} rank={i}");
            }
        }
    }

    /// スレッド並列でも結果が決定的（2回実行で同一）であることを検証。
    #[test]
    fn deterministic_across_runs() {
        let modules = synth_modules(40);
        let a = optimize(&modules, &[1005], None, &[], &[], 10, 5);
        let b = optimize(&modules, &[1005], None, &[], &[], 10, 5);
        let keys = |r: &OptimizeResult| -> Vec<(usize, i32, Vec<i64>)> {
            r.solutions
                .iter()
                .map(|s| {
                    let mut ks: Vec<i64> = s.modules.iter().map(|m| m.key).collect();
                    ks.sort_unstable();
                    (s.level_sum, s.link_effect, ks)
                })
                .collect()
        };
        assert_eq!(keys(&a), keys(&b));
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
