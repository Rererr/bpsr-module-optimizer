//! モジュール最適化: 所持モジュールから slot_count 枠の組み合わせを全探索し、
//! 「レベル分布 → リンク効果」の辞書式優先度で最良を求める。
//!
//! 評価指標（ゲーム内モジュール画面の「パワーコア効果」「リンク効果」に対応）:
//! - 各属性は slot_count 枠分の値を合計し、閾値 [1,4,8,12,16,20] でレベル(0〜6)化。
//!   閾値はスロット数に依存しない（合計値20でLv6）。
//! - リンク効果（表示用の真値）= 全属性値の合計（画面右上の数値）。
//!
//! 除外は2モード:
//! - ハード除外（`hard_exclude_ids`）: 該当属性を含むモジュールを候補から丸ごと排除。
//! - ソフト除外（`soft_exclude_ids`）: モジュールは候補に残すが、その属性はランキング集計
//!   （Lv6/Lv5数・評価リンク）から除外する。真のリンク効果（表示用）には含まれる。
//!
//! ランキング（辞書式・すべて最大化）は [`RankMode`] で2種類から選べる（既定は `Link`。
//! ユーザーが選び、モード変更後は手動で再検索する。フロント側は変更しただけでは自動
//! 再探索しない設計）:
//!
//! `RankMode::Link`（既定）:
//!   1. 選択属性が結果に存在する数（Lv1以上＝値≥1）。「選んだ属性はできるだけ含める」を最優先で
//!      表現するソフト嗜好。含められる目標は全て含み、含められない目標は黙って除外される
//!      （目標未選択時は全解で 0 のため後続キーが従来どおり支配する）。
//!   2. 選択属性が Lv6 到達した数（選択を優先）
//!   3. Lv6 属性の総数（ソフト除外属性は含まない）
//!   4. 評価リンク `eval_link`（ソフト除外を除いた属性値の合計）
//!   5. Lv5 属性の総数（ソフト除外属性は含まない）
//!   6. ソフト除外属性値の合計 `excl` の最小化（同点タイのみで効く）
//!
//! `RankMode::Lv5`: 成分4と5を入れ替え、Lv5 属性の総数を評価リンクより上位に置く
//!   （Lv6 数が同点のとき、合計値より到達個数を優先したいユーザー向け）。
//!
//! 評価リンクを Lv5 数より上位に置く（`Link`、既定）のは、「Lv6 数が同点のとき、Lv5 到達が
//! もう1個あるだけでリンク効果の大差がひっくり返る」という直感に反した挙動を避けるため
//! （旧実装は Lv5 数 → 全属性レベル合計 → 評価リンク の順で、Lv5 数1個の差がリンク効果
//! 10以上の差を覆すことがあった）。`eval_link` は選択した枠に対して線形（各モジュールの
//! counted 値合計 `w(m)` の単純和）なので、`Link` モードでは Lv6 数が同点になった後の
//! 順位は実質「`eval_link` 最大の組み合わせ」で決まり、`lv5` は `eval_link` まで同点だった
//! 場合のタイブレークとしてのみ効く。これは意図した設計（個数より合計値を優先する）であり、
//! `lv5` の存在意義を薄める副作用として認識している。`Lv5` モードは逆に個数を優先したい
//! ユーザー向けに用意した。2つのモードは一般には互いに他方の1位を再現できない（内部で
//! top-N を保持してフロントで並べ替える案は、必要な N がデータ依存で数百〜数千に達しうると
//! 実測確認済みのため不成立）ので、モードごとに探索し直す設計にしている。
//!
//! 探索方式: C(n, slot_count) の全列挙だが、辞書式に枠を深さ優先で選び、ランキングキーを
//! [`Accum`] で差分更新する（共有プレフィックスの再集計を避ける）。最初の枠を rayon で
//! 並列化し、各タスクが保持したローカル top-k の和集合から全体 top-k を再構成する。
//! 同点キーは combo 昇順で決定的に順序付けるため、スレッド数に依らず結果は再現可能。
//!
//! 性能施策（top-k を一切歪めない厳密な枝刈りのみ。解を消す施策＝k-支配則は不採用。
//! 全列挙した候補一覧を見比べる用途のアプリのため、2位以下が歪むと使い物にならない）:
//! - requirements 途中剪定: 下限Lv要求について、残り枠で足しうる値の健全な上界を用いて
//!   DFS 途中で満たせない部分木を打ち切る（[`AttrBounds`]）。
//! - branch-and-bound（分枝限定法）: 候補を counted 値合計 `w(m)` 降順に並べ替えて探索し
//!   （良い解を早く見つけて TopK を早期に埋めるため）、各 DFS ノードで「残り枠を最も都合よく
//!   埋めた場合に到達しうるキーの健全な上界」を計算する（[`should_prune`]）。TopK が既に
//!   満杯で、この上界が現在の最劣キーを辞書式で厳密に下回るなら、その部分木は絶対に top-k を
//!   改善できないため安全に打ち切る。各キー成分は「残り r 個を suffix から独立に楽観視」した
//!   上界（過小評価は絶対にしない）であり、真の到達可能キー以上であることが保証される。
//!   探索順序は並べ替え後の index 空間で行うが、TopK への投入時に元の index へ写像して
//!   昇順ソートし直すため、combo のタイブレーク（元index昇順）と最終結果は不変。

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};

/// 属性レベルの閾値（値 >= 閾値 でその段に到達）。
const ATTR_THRESHOLDS: [i32; 6] = [1, 4, 8, 12, 16, 20];

#[inline]
pub(crate) fn level_of(v: i32) -> usize {
    ATTR_THRESHOLDS.iter().take_while(|&&t| v >= t).count()
}

/// n 個から k 個を選ぶ組み合わせ数 C(n, k)。中間計算を u128 で行い桁溢れを避ける。
pub(crate) fn n_choose_k(n: usize, k: usize) -> u64 {
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
/// GPU探索（optimizer_gpu）もこの密表現をそのままカーネルへアップロードするため pub(crate)。
#[derive(Clone)]
pub(crate) struct Cand {
    /// (属性の密インデックス, 値)。
    pub(crate) parts: Vec<(u32, i32)>,
}

/// DFS で差分更新するランキング集計。add/remove は「触れた属性数」に比例した O(1) 相当で
/// キー要素（選択存在数・選択Lv6数・Lv6数・評価リンク・Lv5数・ソフト除外合計）を保つ。
/// ソフト除外属性（`soft_excl_mask`）はレベル遷移の簿記（lv6/lv5/sel_lv6/eval_link）
/// に一切混ぜず、`excl` にのみ値を加減算する。
///
/// GPU探索（optimizer_gpu）は append バッファから読み戻した combo の厳密キー再計算に
/// `new`/`add`/`key` を使う（`remove` は DFS の後退にのみ使うため非公開のまま）。
pub(crate) struct Accum {
    totals: Vec<i32>,
    lv6: usize,
    lv5: usize,
    sel_lv6: usize,
    /// 選択（目標）属性のうち結果に存在する数（合計値≥1＝Lv1以上）。ソフト嗜好の最優先キー。
    sel_present: usize,
    /// 評価対象リンク（ソフト除外を除いた counted 属性値の合計）。
    eval_link: i32,
    /// ソフト除外属性値の合計（最小化対象）。
    excl: i32,
}

impl Accum {
    pub(crate) fn new(n_attr: usize) -> Self {
        Self {
            totals: vec![0; n_attr],
            lv6: 0,
            lv5: 0,
            sel_lv6: 0,
            sel_present: 0,
            eval_link: 0,
            excl: 0,
        }
    }

    #[inline]
    pub(crate) fn add(&mut self, cand: &Cand, selected_mask: &[bool], soft_excl_mask: &[bool]) {
        for &(idx, val) in &cand.parts {
            let idx = idx as usize;
            if soft_excl_mask[idx] {
                self.excl += val;
                continue;
            }
            let old = self.totals[idx];
            let new = old + val;
            let old_lv = level_of(old);
            let new_lv = level_of(new);
            if new_lv != old_lv {
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
                    // 値は正なので old_lv==0（不在）からの遷移は必ず存在(Lv1+)化する。
                    if old_lv == 0 {
                        self.sel_present += 1;
                    }
                }
            }
            self.totals[idx] = new;
            self.eval_link += val;
        }
    }

    #[inline]
    fn remove(&mut self, cand: &Cand, selected_mask: &[bool], soft_excl_mask: &[bool]) {
        for &(idx, val) in &cand.parts {
            let idx = idx as usize;
            if soft_excl_mask[idx] {
                self.excl -= val;
                continue;
            }
            let cur = self.totals[idx];
            let new = cur - val;
            let cur_lv = level_of(cur);
            let new_lv = level_of(new);
            if cur_lv != new_lv {
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
                    // 除去で合計が0（不在）へ戻る遷移は存在数を1減らす。
                    if new_lv == 0 {
                        self.sel_present -= 1;
                    }
                }
            }
            self.totals[idx] = new;
            self.eval_link -= val;
        }
    }

    #[inline]
    pub(crate) fn key(&self, mode: RankMode) -> Key {
        let sel_present = self.sel_present as i64;
        let sel_lv6 = self.sel_lv6 as i64;
        let lv6 = self.lv6 as i64;
        let lv5 = self.lv5 as i64;
        let eval_link = self.eval_link as i64;
        // 最小化したいので符号反転して格納する（大きいほど良い、の一様な意味を保つ）。
        let neg_excl = -(self.excl as i64);
        match mode {
            RankMode::Link => [sel_present, sel_lv6, lv6, eval_link, lv5, neg_excl],
            RankMode::Lv5 => [sel_present, sel_lv6, lv6, lv5, eval_link, neg_excl],
        }
    }
}

/// top-k 保持用の要素。全順序 = キー昇順 → combo 降順（＝ goodness: キー降順・combo 昇順）。
/// これにより同点キーは辞書式に小さい combo を優先し、スレッド数に依らず結果が決定的になる。
/// GPU探索（optimizer_gpu）も CPU シードと GPU 側結果を同じ比較器でマージするため pub(crate)。
pub(crate) struct Ranked {
    pub(crate) key: Key,
    pub(crate) combo: Vec<u32>,
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
/// GPU探索（optimizer_gpu）は CPU シードと GPU 由来の厳密再計算済み combo を同じ TopK へ
/// マージし、チャンク間の閾値更新にも `worst_key` を使う。
pub(crate) struct TopK {
    heap: BinaryHeap<Reverse<Ranked>>,
    cap: usize,
}

impl TopK {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            heap: BinaryHeap::new(),
            cap,
        }
    }

    /// 現在の combo（path）とキーを候補として投入する。combo の確保は採用時のみ。
    #[inline]
    pub(crate) fn offer(&mut self, key: Key, path: &[u32]) {
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

    pub(crate) fn into_vec(self) -> Vec<Ranked> {
        self.heap.into_iter().map(|Reverse(r)| r).collect()
    }

    /// TopK が満杯の時のみ、現在の最劣キーを返す（未充填なら枝刈りの根拠がないため None）。
    #[inline]
    pub(crate) fn worst_key(&self) -> Option<Key> {
        if self.heap.len() < self.cap {
            return None;
        }
        self.heap.peek().map(|Reverse(r)| r.key)
    }
}

/// 値配列から suffix 上位 `r` 個和テーブル（`table[s][r]`、s=0..=n, r=0..=slot_count）を
/// 構築する共通ヘルパー。r=0 の行は常に0（leaf・残り枠0での厳密判定と一致）。
/// requirements 判定・B&B の各上界（W_r・属性ごとの A_{a,r}）で共有する。
fn build_suffix_topr_table(values: &[i32], slot_count: usize) -> Vec<Vec<i32>> {
    let n = values.len();
    let mut table = vec![vec![0i32; slot_count + 1]; n + 1];
    let mut top: Vec<i32> = Vec::with_capacity(slot_count);
    for s in (0..n).rev() {
        let v = values[s];
        let pos = top.partition_point(|&x| x > v);
        top.insert(pos, v);
        top.truncate(slot_count);
        let row = &mut table[s];
        let mut running = 0i32;
        for (r, slot) in row.iter_mut().enumerate().skip(1) {
            if r <= top.len() {
                running += top[r - 1];
            }
            *slot = running;
        }
    }
    table
}

/// スカラー値（モジュール1件あたりの counted 値合計 w(m) など）の suffix 上位r個和テーブル。
struct SuffixSum {
    table: Vec<Vec<i32>>,
}

impl SuffixSum {
    fn build(values: &[i32], slot_count: usize) -> Self {
        Self {
            table: build_suffix_topr_table(values, slot_count),
        }
    }

    #[inline]
    fn topr(&self, start: usize, r: usize) -> i32 {
        self.table[start][r]
    }
}

/// 属性ごとの suffix 上位r個和テーブル `A_{a,r}(s)`。requirements の下限Lv判定と、
/// B&B の sel6/lv6/lv5 上界計算の両方で共有する（ソフト除外属性の行は未使用＝空Vec）。
struct AttrBounds {
    table: Vec<Vec<Vec<i32>>>,
}

impl AttrBounds {
    fn build(cands: &[Cand], n_attr: usize, soft_excl_mask: &[bool], slot_count: usize) -> Self {
        let table: Vec<Vec<Vec<i32>>> = (0..n_attr)
            .into_par_iter()
            .map(|a| {
                if soft_excl_mask[a] {
                    return Vec::new(); // ソフト除外属性は counted 集計に使わないため未構築。
                }
                let values: Vec<i32> = cands
                    .iter()
                    .map(|c| {
                        c.parts
                            .iter()
                            .filter(|&&(pidx, _)| pidx as usize == a)
                            .map(|&(_, v)| v)
                            .sum()
                    })
                    .collect();
                build_suffix_topr_table(&values, slot_count)
            })
            .collect();
        Self { table }
    }

    #[inline]
    fn topr(&self, attr_idx: usize, start: usize, r: usize) -> i32 {
        self.table[attr_idx][start][r]
    }
}

/// dfs 呼び出し間で不変な探索コンテキスト。再帰の都度コピーせず参照で渡す。
/// `cands`/`order` は探索順（counted 値合計 `w(m)` 降順）に並べ替え済み。
/// `order[i]` = 探索順 index `i` に対応する元の候補配列（`candidates`）でのインデックス。
struct SearchCtx<'a> {
    slot_count: usize,
    n: usize,
    cands: &'a [Cand],
    order: &'a [usize],
    selected_mask: &'a [bool],
    soft_excl_mask: &'a [bool],
    required_idxs: &'a [(usize, usize)],
    attr_bounds: &'a AttrBounds,
    /// W_r: 探索順 suffix の counted 値合計 `w(m)` 上位r個和（評価リンク上界に使う）。
    w_bound: &'a SuffixSum,
    /// counted（非ソフト除外）属性の密インデックス一覧。
    counted_attr_idxs: &'a [usize],
    /// 選択（目標）属性の密インデックス一覧。
    selected_attr_idxs: &'a [usize],
    /// requirements 途中剪定を全深さで行うか（false なら leaf でのみ判定＝従来と同じ）。
    use_requirement_pruning: bool,
    /// B&B 上界剪定を行うか（false なら TopK 満杯時の枝刈りを行わず全列挙する）。
    use_bnb_pruning: bool,
    /// ランキング順序モード（[`Key`] の成分並びを決める）。
    mode: RankMode,
}

/// 現在のノード（残り枠 `r`、次候補 index `start`、現在の集計 `acc`）から到達しうるキーの
/// 健全な上界を計算し、`worst`（TopK 内の最劣キー）を辞書式で厳密に下回るなら true
/// （＝この部分木は打ち切ってよい）を返す。
///
/// 各成分は「残り r 個を suffix から属性ごとに独立に楽観視」した上界であり、真の到達可能な
/// 値以上であることが保証される（過小評価は決してしない）。層ごとに遅延評価し、上位の成分
/// だけで大小が決まればそれ以降の成分（O(n_attr)）は計算しない。
///
/// 各成分の上界式自体は [`RankMode`] に依存しない（「残り r 個を独立に楽観視」という性質は
/// 成分の並び順と無関係）。成分0〜2（選択存在数・選択Lv6数・Lv6数）は両モードで並び位置が
/// 共通なので分岐なしで共有し、成分3・4（評価リンク上界・Lv5数上界）だけモードに応じて
/// 比較順序を入れ替える（[`Key`]/[`Accum::key`] と同じ並び）。
fn should_prune(acc: &Accum, ctx: &SearchCtx, start: usize, r: usize, worst: &Key) -> bool {
    let reach = |a: usize, threshold_lv: usize| -> bool {
        level_of(acc.totals[a] + ctx.attr_bounds.topr(a, start, r)) >= threshold_lv
    };

    // 成分0: 選択属性の存在数(Lv1到達)上界。現在不在の選択属性のうち、残り枠で値≥1に到達しうる
    // ものを全て存在化できると楽観視する（過小評価しない健全な上界）。両モード共通の並び位置。
    let ub_present = acc.sel_present
        + ctx
            .selected_attr_idxs
            .iter()
            .filter(|&&a| level_of(acc.totals[a]) == 0 && reach(a, 1))
            .count();
    match (ub_present as i64).cmp(&worst[0]) {
        Ordering::Less => return true,
        Ordering::Greater => return false,
        Ordering::Equal => {}
    }

    // 成分1: 選択属性の Lv6 到達数上界。両モード共通の並び位置。
    let ub_sel6 = acc.sel_lv6
        + ctx
            .selected_attr_idxs
            .iter()
            .filter(|&&a| level_of(acc.totals[a]) < 6 && reach(a, 6))
            .count();
    match (ub_sel6 as i64).cmp(&worst[1]) {
        Ordering::Less => return true,
        Ordering::Greater => return false,
        Ordering::Equal => {}
    }

    // 成分2: Lv6 属性総数上界（counted 属性のみ）。両モード共通の並び位置。
    let ub_lv6 = acc.lv6
        + ctx
            .counted_attr_idxs
            .iter()
            .filter(|&&a| level_of(acc.totals[a]) < 6 && reach(a, 6))
            .count();
    match (ub_lv6 as i64).cmp(&worst[2]) {
        Ordering::Less => return true,
        Ordering::Greater => return false,
        Ordering::Equal => {}
    }

    // 成分3・4: 評価リンク上界（W_r）と Lv5 属性総数上界。モードにより比較順序が入れ替わる。
    match ctx.mode {
        RankMode::Link => {
            let ub_lk = acc.eval_link + ctx.w_bound.topr(start, r);
            match (ub_lk as i64).cmp(&worst[3]) {
                Ordering::Less => return true,
                Ordering::Greater => return false,
                Ordering::Equal => {}
            }
            // 既に Lv5 の属性は Lv6 へ抜ける可能性を無視して現状維持を仮定し
            // （成分2側で楽観視済み）、Lv4以下からの新規到達のみ加算する。
            let ub_lv5 = acc.lv5
                + ctx
                    .counted_attr_idxs
                    .iter()
                    .filter(|&&a| level_of(acc.totals[a]) <= 4 && reach(a, 5))
                    .count();
            match (ub_lv5 as i64).cmp(&worst[4]) {
                Ordering::Less => return true,
                Ordering::Greater => return false,
                Ordering::Equal => {}
            }
        }
        RankMode::Lv5 => {
            let ub_lv5 = acc.lv5
                + ctx
                    .counted_attr_idxs
                    .iter()
                    .filter(|&&a| level_of(acc.totals[a]) <= 4 && reach(a, 5))
                    .count();
            match (ub_lv5 as i64).cmp(&worst[3]) {
                Ordering::Less => return true,
                Ordering::Greater => return false,
                Ordering::Equal => {}
            }
            let ub_lk = acc.eval_link + ctx.w_bound.topr(start, r);
            match (ub_lk as i64).cmp(&worst[4]) {
                Ordering::Less => return true,
                Ordering::Greater => return false,
                Ordering::Equal => {}
            }
        }
    }

    // 成分5: ソフト除外合計は残り枠で増える一方（非負値）なので、現在値がそのまま上界になる
    // （符号反転して格納。Accum::key と同じ規則で「大きいほど良い」に統一する）。
    -(acc.excl as i64) < worst[5]
}

/// 残り (slot_count - depth) 枠を index `start` 以降から選ぶ再帰列挙。
/// acc は呼び出し前後で不変（各候補を add→再帰→remove）。
/// requirements の下限Lv判定は残り枠での健全な上界を使い、leaf（r=0）では厳密な判定と一致する。
/// B&B 上界剪定は解を一切消さない（`should_prune` が健全な上界である限り、真に top-k に
/// 入りうる部分木を誤って刈ることはない）。
fn dfs(
    depth: usize,
    start: usize,
    ctx: &SearchCtx,
    acc: &mut Accum,
    path: &mut [u32],
    top: &mut TopK,
) {
    let r = ctx.slot_count - depth;

    // requirements 途中剪定。r==0（leaf）は常に厳密判定。
    if r == 0 || ctx.use_requirement_pruning {
        for &(idx, lv) in ctx.required_idxs {
            let upper = ctx.attr_bounds.topr(idx, start, r);
            if level_of(acc.totals[idx] + upper) < lv {
                return;
            }
        }
    }

    // B&B 上界剪定。TopK が満杯（worst が定まっている）の時のみ意味を持つ。
    if r > 0 && ctx.use_bnb_pruning {
        if let Some(worst) = top.worst_key() {
            if should_prune(acc, ctx, start, r, &worst) {
                return;
            }
        }
    }

    if depth == ctx.slot_count {
        // combo のタイブレークは元index昇順で行うため、探索順indexを元indexへ写像しソートする。
        let mut combo: Vec<u32> = path.iter().map(|&i| ctx.order[i as usize] as u32).collect();
        combo.sort_unstable();
        top.offer(acc.key(ctx.mode), &combo);
        return;
    }
    // この深さで選べる最大インデックス（右側に残り枠分の余地を残す）。
    let last = ctx.n - (ctx.slot_count - depth);
    for i in start..=last {
        acc.add(&ctx.cands[i], ctx.selected_mask, ctx.soft_excl_mask);
        path[depth] = i as u32;
        dfs(depth + 1, i + 1, ctx, acc, path, top);
        acc.remove(&ctx.cands[i], ctx.selected_mask, ctx.soft_excl_mask);
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
    /// ソフト除外指定された属性か。true の場合、lv6_count 等の集計から除外されている
    /// （値・レベル自体はそのまま表示用に保持する）。
    pub soft_excluded: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Solution {
    pub modules: Vec<Module>,
    /// リンク効果（全属性値の合計・表示用の真値）。ソフト除外属性の値も含む。
    pub link_effect: i32,
    /// 評価対象スコア（ソフト除外属性を除いた counted 属性値の合計）。ランキングキーの実体。
    pub eval_link: i32,
    /// Lv6 到達数（ソフト除外属性は含まない）。
    pub lv6_count: usize,
    /// Lv5 到達数（ソフト除外属性は含まない）。
    pub lv5_count: usize,
    /// 選択属性のうち Lv6 に到達した数。
    pub selected_lv6: usize,
    /// 選択属性のうち結果に存在する数（Lv1以上）。ランキング最優先キーの実体。
    pub selected_present: usize,
    /// 全属性の内訳（レベル降順、ソフト除外属性も含む）。
    pub breakdown: Vec<AttrBreakdown>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OptimizeResult {
    pub solutions: Vec<Solution>,
    pub candidate_count: usize,
    pub combinations: u64,
    /// 実際に使われた探索エンジン。"cpu" | "gpu"。
    /// 通常ビルドは常に "cpu"。GPUビルド（feature "gpu"）は GPU 探索が成功しマージまで
    /// 完遂した場合のみ "gpu"（optimizer_gpu の cpu_fallback を通った全経路は "cpu"）。
    pub engine: String,
}

/// ランキング順序モード。ユーザーが選び、変更後は手動で再検索する（モード変更だけでは
/// 自動再探索しない。フロント側の設計）。
/// - `Link`（既定）: Lv6数の次に評価リンク（合計値）を優先し、Lv5数はその後に回す。
/// - `Lv5`: Lv6数の次にLv5到達数（個数）を優先し、評価リンクはその後に回す。
///
/// 実測で確認済みの通り、どちらのモードで探索しても「もう一方のモードでの真の1位」を
/// 一般には再現できない（内部でtop-Nを保持してフロントで並べ替える案は、必要なNが
/// データ依存で数百〜数千に達するため不成立と確認済み）。そのためモードごとに探索し直す
/// 設計を取る。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankMode {
    #[default]
    Link,
    Lv5,
}

/// 辞書式比較キー（すべて最大化。要素はすべて `i64` に統一した固定長配列）。
/// ソフト除外合計は最小化したいので、格納時点で符号を反転（`-excl`）しておくことで
/// 単純な数値比較のまま「大きいほど良い」の一様な意味を保つ（`Reverse` ラッパー不要）。
///
/// 要素順は `mode`（[`RankMode`]）で変わる（[`Accum::key`] 参照）:
/// - `Link`: (選択存在数, 選択Lv6数, Lv6数, 評価リンク, Lv5数, -ソフト除外合計)
/// - `Lv5` : (選択存在数, 選択Lv6数, Lv6数, Lv5数, 評価リンク, -ソフト除外合計)
///
/// 旧実装はタプル `(usize, usize, usize, i32, usize, Reverse<i32>)` で、評価リンク/Lv5数の
/// 型が異なることが「参照実装 `naive_ranked` が並び替えを反映し忘れる事故をコンパイル時に
/// 防ぐガード」を兼ねていた。固定長配列化でその保証は失われるため、`naive_ranked` を
/// 両モード対応にしたブルートフォース突き合わせ（`matches_naive_reference` 等）で
/// テスト時に必ず担保すること。
///
/// GPU探索（optimizer_gpu）はシェーダ内のキー packing・CPU側の閾値比較の両方でこの並びに
/// 依存するため pub(crate)。
pub(crate) type Key = [i64; 6];

/// 入力のバリデーション。目標属性・下限Lv指定対象が除外属性（ソフト/ハード問わず）と
/// 重複していないかを確認する。黙って片方を優先せず、意味のあるメッセージで拒否する。
fn validate_inputs(
    selected_ids: &[i32],
    hard_exclude_ids: &[i32],
    soft_exclude_ids: &[i32],
    requirements: &[(i32, usize)],
) -> Result<(), String> {
    let excluded: HashSet<i32> = hard_exclude_ids
        .iter()
        .chain(soft_exclude_ids.iter())
        .copied()
        .collect();

    let bad_targets: Vec<i32> = selected_ids
        .iter()
        .filter(|id| excluded.contains(id))
        .copied()
        .collect();
    if !bad_targets.is_empty() {
        return Err(format!(
            "目標属性と除外属性が重複しています（属性ID: {bad_targets:?}）。\
             同じ属性を目標と除外の両方に指定することはできません。"
        ));
    }

    let bad_reqs: Vec<i32> = requirements
        .iter()
        .filter(|&&(_, lv)| lv > 0)
        .map(|&(id, _)| id)
        .filter(|id| excluded.contains(id))
        .collect();
    if !bad_reqs.is_empty() {
        return Err(format!(
            "下限Lv指定と除外属性が重複しています（属性ID: {bad_reqs:?}）。\
             除外指定した属性に下限Lvは設定できません。"
        ));
    }

    Ok(())
}

/// 最適化本体。
/// - `slot_count`: 装備枠数（この個数の組み合わせを全探索する）。
/// - `category`: Some なら該当カテゴリのモジュールのみを枠に使う。
/// - `hard_exclude_ids`: いずれかを含むモジュールは候補から丸ごと除外。
/// - `soft_exclude_ids`: モジュールは候補に残すが、該当属性はランキング集計から除外する。
/// - `requirements`: 属性ごとの下限レベル要求 `[(attr_id, min_level)]`。
/// - `mode`: ランキング順序モード（[`RankMode`]）。
///
/// 目標属性・下限Lv対象が除外属性（ソフト/ハード）と重複する場合はエラーを返す。
#[allow(clippy::too_many_arguments)]
pub fn optimize(
    modules: &[Module],
    selected_ids: &[i32],
    category: Option<&str>,
    hard_exclude_ids: &[i32],
    soft_exclude_ids: &[i32],
    requirements: &[(i32, usize)],
    top_k: usize,
    slot_count: usize,
    mode: RankMode,
) -> Result<OptimizeResult, String> {
    optimize_with_opts(
        modules,
        selected_ids,
        category,
        hard_exclude_ids,
        soft_exclude_ids,
        requirements,
        top_k,
        slot_count,
        mode,
        true,
        true,
    )
}

/// [`prepare`] の出力: 候補フィルタ（カテゴリ／ハード除外）→ 属性密インデックス化 →
/// w(m) 降順ソートまでを終えた探索前状態。[`search_cpu`]/[`assemble`] の両方が必要とする
/// 情報を保持する。CPU/GPU 探索（optimizer_gpu）で共有するため pub(crate)。
pub(crate) struct Prepared<'a> {
    /// カテゴリ絞り込み・ハード除外適用後の候補（元の modules 順）。
    /// [`Ranked::combo`] の各要素はこの配列へのインデックス。
    pub(crate) candidates: Vec<&'a Module>,
    /// w(m) 降順に並べ替えた密表現。`cands[i]` は探索順 index `i` に対応する。
    pub(crate) cands: Vec<Cand>,
    /// order[i] = 探索順 index i に対応する `candidates` でのインデックス。
    pub(crate) order: Vec<usize>,
    pub(crate) n_attr: usize,
    pub(crate) selected_mask: Vec<bool>,
    pub(crate) soft_excl_mask: Vec<bool>,
    /// 属性ごとの下限レベル要求（密インデックスへ解決済み）。
    pub(crate) required_idxs: Vec<(usize, usize)>,
    pub(crate) candidate_count: usize,
    pub(crate) combinations: u64,
    /// true なら探索を行わず必ず空の結果（候補不足、または必須属性がどの候補にも存在しない）。
    pub(crate) trivially_empty: bool,
    pub(crate) selected_ids: &'a [i32],
    pub(crate) soft_exclude_ids: &'a [i32],
}

impl<'a> Prepared<'a> {
    /// 指定した探索順 position（`cands`/`order` への index、0..cands.len()）の集合だけに
    /// 絞った Prepared を作る（GPU探索の CPU シード用）。position は呼び出し側で重複を
    /// 排除し、通常は昇順で渡すこと（探索順 index の昇順 = w(m) 降順なので、その順序性を
    /// シード側でも保てば [`search_cpu`] の B&B 枝刈りが効きやすい。ただし正当性は順序に
    /// 依存しない）。`candidates`/`required_idxs` 等は元の密インデックス空間を共有するため
    /// 複製のみで済む。
    // feature "gpu" が無効なビルドでは呼び出し元（optimizer_gpu）ごと存在しないため未使用警告
    // が出るが、CPU探索の挙動には影響しない実装詳細のメソッドなので allow で抑制する。
    #[cfg_attr(not(feature = "gpu"), allow(dead_code))]
    pub(crate) fn subset(&self, positions: &[usize]) -> Prepared<'a> {
        Prepared {
            candidates: self.candidates.clone(),
            cands: positions.iter().map(|&p| self.cands[p].clone()).collect(),
            order: positions.iter().map(|&p| self.order[p]).collect(),
            n_attr: self.n_attr,
            selected_mask: self.selected_mask.clone(),
            soft_excl_mask: self.soft_excl_mask.clone(),
            required_idxs: self.required_idxs.clone(),
            candidate_count: self.candidate_count,
            combinations: self.combinations,
            trivially_empty: self.trivially_empty,
            selected_ids: self.selected_ids,
            soft_exclude_ids: self.soft_exclude_ids,
        }
    }
}

/// 候補フィルタ（カテゴリ／ハード除外）→ 属性密インデックス化 → w(m) 降順ソートまでを行う。
/// [`search_cpu`]/[`assemble`] の両方が必要とする状態を [`Prepared`] にまとめて返す。
/// 探索方式（CPU DFS / GPU 全数評価）に依存しないロジックのため、GPU探索
/// （optimizer_gpu）もこの関数をそのまま呼び出して共有する。
pub(crate) fn prepare<'a>(
    modules: &'a [Module],
    selected_ids: &'a [i32],
    category: Option<&str>,
    hard_exclude_ids: &[i32],
    soft_exclude_ids: &'a [i32],
    requirements: &[(i32, usize)],
    slot_count: usize,
) -> Result<Prepared<'a>, String> {
    validate_inputs(
        selected_ids,
        hard_exclude_ids,
        soft_exclude_ids,
        requirements,
    )?;

    // カテゴリ絞り込み・ハード除外属性を含むモジュールを除いた候補。
    let candidates: Vec<&Module> = modules
        .iter()
        .filter(|m| category.is_none_or(|c| c == "all" || m.category == c))
        .filter(|m| {
            !m.parts
                .iter()
                .any(|p| hard_exclude_ids.contains(&p.attr_id))
        })
        .collect();

    let n = candidates.len();
    if slot_count == 0 || n < slot_count {
        return Ok(Prepared {
            candidates,
            cands: Vec::new(),
            order: Vec::new(),
            n_attr: 0,
            selected_mask: Vec::new(),
            soft_excl_mask: Vec::new(),
            required_idxs: Vec::new(),
            candidate_count: n,
            combinations: 0,
            trivially_empty: true,
            selected_ids,
            soft_exclude_ids,
        });
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
    let selected_mask: Vec<bool> = idx_to_id
        .iter()
        .map(|id| selected_ids.contains(id))
        .collect();
    let soft_excl_mask: Vec<bool> = idx_to_id
        .iter()
        .map(|id| soft_exclude_ids.contains(id))
        .collect();

    // 属性ごとの下限レベル要求を idx へ解決。候補に存在しない属性が必須なら達成不能。
    // （ソフト/ハード除外属性が requirements に含まれるのは validate_inputs で既に弾かれている）
    let mut required_idxs: Vec<(usize, usize)> = Vec::new();
    for &(attr_id, lv) in requirements {
        if lv == 0 {
            continue;
        }
        match id_to_idx.get(&attr_id) {
            Some(&idx) => required_idxs.push((idx, lv)),
            None => {
                return Ok(Prepared {
                    candidates,
                    cands: Vec::new(),
                    order: Vec::new(),
                    n_attr,
                    selected_mask,
                    soft_excl_mask,
                    required_idxs: Vec::new(),
                    candidate_count: n,
                    combinations: 0,
                    trivially_empty: true,
                    selected_ids,
                    soft_exclude_ids,
                });
            }
        }
    }

    // 組み合わせ総数は直接算出（下限Lv要求で除外される分も含む＝旧実装と同義）。
    let combinations = n_choose_k(n, slot_count);

    // B&B の探索順を決めるため、各候補の counted 値合計 w(m)（=非ソフト除外パーツの値合計）を
    // 元の候補順で求める。w(m) 降順（同点は元index昇順）に並べ替えると良い解を早く見つけやすく
    // なり、B&B の枝刈りが早期から効く（soft_excl_mask による除外は dense idx でも元 attr_id
    // でも同義）。
    let w: Vec<i32> = candidates
        .iter()
        .map(|m| {
            m.parts
                .iter()
                .filter(|p| !soft_exclude_ids.contains(&p.attr_id))
                .map(|p| p.value)
                .sum()
        })
        .collect();

    // order[i] = 探索順 index i に対応する元の候補配列(candidates)でのインデックス。
    // requirements が1つ以上ある場合、いずれかの required 属性を持つ候補（担い手）を
    // 先頭ブロックへ、残りを後方ブロックへ分けた上で、各ブロック内は従来どおり w(m) 降順
    // （同点は元index昇順）にソートする。GPU Kernel P の requirements 実現可能性チェック
    // （attr_suffix_max、suffix の単一最大値）は「残り候補の suffix に担い手が1人もいない」
    // ケースで最も強く効くが、担い手が探索順に散在していると、ほとんどのプレフィックスの
    // suffix に何らかの担い手が残ってしまい枝刈りが発火しない。担い手を先頭に集約すると、
    // 担い手を1つも含まないプレフィックス（探索順で後方の候補のみから構成される。
    // n_carriers=C とすると全体の C(n-C,kp)/C(n,kp) 相当、多くの場合9割超）は、suffix にも
    // 担い手が存在しなくなるためこの枝刈りが厳密十分条件として機能する（実測で requirements
    // 条件の枝刈り率が大幅改善）。requirements が無い場合は現在の順序を一切変えない
    // （plain系のタイブレーク挙動を保つ）。
    let mut order: Vec<usize> = (0..n).collect();
    if required_idxs.is_empty() {
        order.sort_by(|&a, &b| w[b].cmp(&w[a]).then(a.cmp(&b)));
    } else {
        let required_attr_ids: HashSet<i32> = requirements
            .iter()
            .filter(|&&(_, lv)| lv > 0)
            .map(|&(attr_id, _)| attr_id)
            .collect();
        let is_carrier: Vec<bool> = candidates
            .iter()
            .map(|m| {
                m.parts
                    .iter()
                    .any(|p| required_attr_ids.contains(&p.attr_id))
            })
            .collect();
        order.sort_by(|&a, &b| {
            is_carrier[b]
                .cmp(&is_carrier[a])
                .then(w[b].cmp(&w[a]))
                .then(a.cmp(&b))
        });
    }

    // 密インデックス変換と並べ替えを同時に行う（探索は以後この並び順で行う）。
    let cands: Vec<Cand> = order
        .iter()
        .map(|&oi| Cand {
            parts: candidates[oi]
                .parts
                .iter()
                .map(|p| (id_to_idx[&p.attr_id] as u32, p.value))
                .collect(),
        })
        .collect();

    Ok(Prepared {
        candidates,
        cands,
        order,
        n_attr,
        selected_mask,
        soft_excl_mask,
        required_idxs,
        candidate_count: n,
        combinations,
        trivially_empty: false,
        selected_ids,
        soft_exclude_ids,
    })
}

/// AttrBounds/SuffixSum（B&B 上界剪定用）を構築し、rayon 並列 DFS で top-k を求める。
/// `prepared` は [`prepare`] の出力（w(m) 降順ソート済み、`prepared.cands.len()` 件を探索）。
/// GPU探索（optimizer_gpu）は「w(m)降順上位min(n,60)件」∪「requirements属性ごとの値上位10件」
/// （`build_seed_positions`）に絞った [`Prepared::subset`] をここへ渡すことで、GPU 全数評価の
/// 足切り閾値を得る CPU 厳密シードとしても使う。
pub(crate) fn search_cpu(
    prepared: &Prepared,
    top_k: usize,
    slot_count: usize,
    mode: RankMode,
    use_requirement_pruning: bool,
    use_bnb_pruning: bool,
) -> Vec<Ranked> {
    if prepared.trivially_empty {
        return Vec::new();
    }

    let n = prepared.cands.len();
    let cands = &prepared.cands;
    let order = &prepared.order;
    let selected_mask = &prepared.selected_mask;
    let soft_excl_mask = &prepared.soft_excl_mask;
    let n_attr = prepared.n_attr;

    // B&B の上界計算に使う counted 値合計 w(m)（探索順）。cands は既に w(m) 降順ソート済みの
    // 密表現のため、soft_excl_mask で除外パーツを弾いて直接再計算すれば prepare 時点の値と
    // 一致する。
    let w_sorted: Vec<i32> = cands
        .iter()
        .map(|c| {
            c.parts
                .iter()
                .filter(|&&(idx, _)| !soft_excl_mask[idx as usize])
                .map(|&(_, v)| v)
                .sum()
        })
        .collect();

    let attr_bounds = AttrBounds::build(cands, n_attr, soft_excl_mask, slot_count);
    let w_bound = SuffixSum::build(&w_sorted, slot_count);
    let counted_attr_idxs: Vec<usize> = (0..n_attr).filter(|&a| !soft_excl_mask[a]).collect();
    let selected_attr_idxs: Vec<usize> = (0..n_attr).filter(|&a| selected_mask[a]).collect();

    let ctx = SearchCtx {
        slot_count,
        n,
        cands,
        order,
        selected_mask,
        soft_excl_mask,
        required_idxs: &prepared.required_idxs,
        attr_bounds: &attr_bounds,
        w_bound: &w_bound,
        counted_attr_idxs: &counted_attr_idxs,
        selected_attr_idxs: &selected_attr_idxs,
        use_requirement_pruning,
        use_bnb_pruning,
        mode,
    };

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
            acc.add(&cands[i0], selected_mask, soft_excl_mask);
            path[0] = i0 as u32;
            dfs(1, i0 + 1, &ctx, &mut acc, &mut path, &mut top);
            top.into_vec()
        })
        .reduce(Vec::new, |mut a, mut b| {
            a.append(&mut b);
            a
        });

    // goodness 降順（キー降順 → combo 昇順）に並べて上位を採る。
    ranked.sort_by(|a, b| b.cmp(a));
    ranked.truncate(top_k);
    ranked
}

/// [`search_cpu`]（または GPU 探索でマージ済みの結果）が返した Ranked から
/// [`OptimizeResult`] を組み立てる。combo は探索順ではなく `prepared.candidates` への
/// 元インデックス（探索終了時点で元index昇順へ写像・ソート済み）。
/// `engine`: 実際に使われた探索エンジン（"cpu" | "gpu"）。呼び出し元が確定した値を渡す
/// （GPU探索 optimizer_gpu は成功時のみ "gpu"、trivially_empty・フォールバック時は "cpu"）。
pub(crate) fn assemble(prepared: &Prepared, ranked: Vec<Ranked>, engine: &str) -> OptimizeResult {
    let solutions = ranked
        .into_iter()
        .map(|r| {
            let mods: Vec<Module> = r
                .combo
                .iter()
                .map(|&i| prepared.candidates[i as usize].clone())
                .collect();
            build_solution(mods, prepared.selected_ids, prepared.soft_exclude_ids)
        })
        .collect();

    OptimizeResult {
        solutions,
        candidate_count: prepared.candidate_count,
        combinations: prepared.combinations,
        engine: engine.to_string(),
    }
}

/// `optimize` の実体。requirements 途中剪定・B&B 上界剪定を個別に on/off できる
/// （どちらも最良キーを変えない性能施策のため、テストで等価性を検証するために分離している）。
/// [`prepare`] → [`search_cpu`] → [`assemble`] を順に呼ぶだけ（GPU探索 optimizer_gpu も
/// 同じ3関数を共有し、search_cpu の代わりに GPU カーネルで全数評価する）。
#[allow(clippy::too_many_arguments)]
fn optimize_with_opts(
    modules: &[Module],
    selected_ids: &[i32],
    category: Option<&str>,
    hard_exclude_ids: &[i32],
    soft_exclude_ids: &[i32],
    requirements: &[(i32, usize)],
    top_k: usize,
    slot_count: usize,
    mode: RankMode,
    use_requirement_pruning: bool,
    use_bnb_pruning: bool,
) -> Result<OptimizeResult, String> {
    let prepared = prepare(
        modules,
        selected_ids,
        category,
        hard_exclude_ids,
        soft_exclude_ids,
        requirements,
        slot_count,
    )?;
    let ranked = search_cpu(
        &prepared,
        top_k,
        slot_count,
        mode,
        use_requirement_pruning,
        use_bnb_pruning,
    );
    Ok(assemble(&prepared, ranked, "cpu"))
}

/// 選択された各モジュールから内訳と各指標を再構成して Solution を作る。
/// lv6_count 等の集計・eval_link はソフト除外属性を含めない（ランキングキーと一致させる）。
/// link_effect（表示用の真値）はソフト除外属性も含む全属性値の合計。
fn build_solution(mods: Vec<Module>, selected_ids: &[i32], soft_exclude_ids: &[i32]) -> Solution {
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
            soft_excluded: soft_exclude_ids.contains(&attr_id),
        })
        .collect();
    // レベル降順 → 値降順 → 選択優先。
    breakdown.sort_by(|a, b| {
        b.level
            .cmp(&a.level)
            .then(b.value.cmp(&a.value))
            .then(b.selected.cmp(&a.selected))
    });

    let lv6_count = breakdown
        .iter()
        .filter(|b| !b.soft_excluded && b.level == 6)
        .count();
    let lv5_count = breakdown
        .iter()
        .filter(|b| !b.soft_excluded && b.level == 5)
        .count();
    let selected_lv6 = breakdown
        .iter()
        .filter(|b| !b.soft_excluded && b.selected && b.level == 6)
        .count();
    let selected_present = breakdown
        .iter()
        .filter(|b| !b.soft_excluded && b.selected && b.level >= 1)
        .count();
    let eval_link = breakdown
        .iter()
        .filter(|b| !b.soft_excluded)
        .map(|b| b.value)
        .sum();
    let link_effect = breakdown.iter().map(|b| b.value).sum();

    Solution {
        modules: mods,
        link_effect,
        eval_link,
        lv6_count,
        lv5_count,
        selected_lv6,
        selected_present,
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

    /// RankMode の serde snake_case 表現がフロント側の文字列リテラルと一致することを固定する
    /// 回帰テスト（"link"/"lv5"。TypeScript 側の型と食い違うと Tauri IPC が実行時に壊れるため）。
    #[test]
    fn rank_mode_serde_snake_case() {
        assert_eq!(serde_json::to_string(&RankMode::Link).unwrap(), "\"link\"");
        assert_eq!(serde_json::to_string(&RankMode::Lv5).unwrap(), "\"lv5\"");
        assert_eq!(
            serde_json::from_str::<RankMode>("\"link\"").unwrap(),
            RankMode::Link
        );
        assert_eq!(
            serde_json::from_str::<RankMode>("\"lv5\"").unwrap(),
            RankMode::Lv5
        );
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
        let res = optimize(&modules, &[], None, &[], &[], &[], 5, 4, RankMode::Link)
            .expect("optimize should succeed");
        let best = &res.solutions[0];
        assert_eq!(best.lv6_count, 1);
        assert_eq!(best.lv5_count, 1);
        assert_eq!(best.link_effect, 52);
        assert_eq!(best.eval_link, 52);
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
        let res = optimize(&modules, &[9], None, &[], &[], &[], 5, 4, RankMode::Link)
            .expect("optimize should succeed");
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
        let res = optimize(
            &modules,
            &[9],
            None,
            &[],
            &[],
            &[(9, 6)],
            5,
            4,
            RankMode::Link,
        )
        .expect("optimize should succeed");
        assert!(res.solutions.is_empty());
        // Lv4必須なら成立。
        let res2 = optimize(
            &modules,
            &[9],
            None,
            &[],
            &[],
            &[(9, 4)],
            5,
            4,
            RankMode::Link,
        )
        .expect("optimize should succeed");
        assert_eq!(res2.solutions.len(), 1);
    }

    #[test]
    fn prefers_including_selected_over_dropping_it() {
        // 2枠。選択属性9を低Lvで含む案 vs. 9を捨てて非選択をLv6×2にする案。
        // 存在数を最優先するため、9を含む案（存在数1）が優先されるはず（Lv6数が減っても）。
        let modules = vec![
            module(1, 5500103, &[(9, 3)]),  // 選択9=Lv1
            module(2, 5500103, &[(5, 20)]), // 非選択=Lv6
            module(3, 5500103, &[(6, 20)]), // 非選択=Lv6
        ];
        let res = optimize(&modules, &[9], None, &[], &[], &[], 5, 2, RankMode::Link)
            .expect("optimize should succeed");
        let best = &res.solutions[0];
        assert_eq!(best.selected_present, 1, "選択属性9が結果に含まれるはず");
        assert!(
            best.modules.iter().any(|m| m.key == 1),
            "9を持つモジュール1が採用されるはず"
        );
        // 存在数を捨ててLv6×2を採る {2,3} は最良ではない。
        let keys: std::collections::BTreeSet<i64> = best.modules.iter().map(|m| m.key).collect();
        assert_ne!(keys, [2, 3].into_iter().collect());
    }

    #[test]
    fn unincludable_selected_is_silently_ignored() {
        // どのモジュールにも存在しない属性を選択しても、解ゼロにはならず（黙って無視され）
        // 通常どおり結果が返る。存在数は 0。
        let modules = vec![
            module(1, 5500103, &[(1, 5)]),
            module(2, 5500103, &[(1, 5)]),
            module(3, 5500103, &[(1, 5)]),
        ];
        let res = optimize(&modules, &[999], None, &[], &[], &[], 5, 2, RankMode::Link)
            .expect("optimize should succeed");
        assert!(
            !res.solutions.is_empty(),
            "含められない目標でも解は消えない"
        );
        assert_eq!(res.solutions[0].selected_present, 0);
    }

    #[test]
    fn maximizes_included_targets_over_more_lv6() {
        // 2枠・目標[7,8,9]。目標を2つ低Lvで含む案 vs. 目標1つ+非選択でLv6×2にする案。
        // 存在数を最優先するため、Lv6数が少なくても目標を2つ含む案が上位に来るはず。
        let modules = vec![
            module(1, 5500103, &[(7, 3)]),  // 目標7=Lv1
            module(2, 5500103, &[(8, 3)]),  // 目標8=Lv1
            module(3, 5500103, &[(9, 20)]), // 目標9=Lv6
            module(4, 5500103, &[(5, 20)]), // 非選択=Lv6
        ];
        let res = optimize(
            &modules,
            &[7, 8, 9],
            None,
            &[],
            &[],
            &[],
            5,
            2,
            RankMode::Link,
        )
        .expect("optimize should succeed");
        let best = &res.solutions[0];
        // 2枠で含められる目標の最大数は2。存在数を最優先するので rank-1 は必ず2。
        assert_eq!(
            best.selected_present, 2,
            "含められる目標2つを最大限含むはず"
        );
        // 目標1つ+非選択Lv6（存在数1・Lv6数2）の {3,4} は、Lv6数が多くても最良ではない。
        let keys: std::collections::BTreeSet<i64> = best.modules.iter().map(|m| m.key).collect();
        assert_ne!(
            keys,
            [3, 4].into_iter().collect(),
            "Lv6数が多くても存在数の少ない案は選ばれない"
        );
        assert!(best.lv6_count <= 1, "存在数優先の結果Lv6数は1以下のはず");
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
        let res = optimize(&modules, &[1], None, &[], &[], &[], 5, 5, RankMode::Link)
            .expect("optimize should succeed");
        let best = &res.solutions[0];
        assert_eq!(best.modules.len(), 5);
        assert_eq!(best.link_effect, 25);
        assert_eq!(best.lv6_count, 1);
        // C(6,5)=6 通り。
        assert_eq!(res.combinations, 6);
    }

    #[test]
    fn validation_rejects_target_exclude_overlap() {
        let modules = vec![module(1, 5500103, &[(9, 5)])];
        // ハード除外と目標が重複。
        let err = optimize(&modules, &[9], None, &[9], &[], &[], 5, 4, RankMode::Link)
            .expect_err("重複はエラーのはず");
        assert!(
            err.contains('9'),
            "エラーメッセージに属性IDを含むべき: {err}"
        );
        // ソフト除外と目標が重複。
        let err2 = optimize(&modules, &[9], None, &[], &[9], &[], 5, 4, RankMode::Link)
            .expect_err("重複はエラーのはず");
        assert!(
            err2.contains('9'),
            "エラーメッセージに属性IDを含むべき: {err2}"
        );
    }

    #[test]
    fn validation_rejects_requirement_on_excluded_attr() {
        let modules = vec![module(1, 5500103, &[(9, 5)])];
        let err = optimize(
            &modules,
            &[],
            None,
            &[],
            &[9],
            &[(9, 3)],
            5,
            4,
            RankMode::Link,
        )
        .expect_err("除外属性への下限Lv指定はエラーのはず");
        assert!(
            err.contains('9'),
            "エラーメッセージに属性IDを含むべき: {err}"
        );
    }

    #[test]
    fn soft_exclude_prefers_more_real_lv6() {
        // A,B,C,D,E はそれぞれ単独モジュールで値20（Lv6相当）。E をソフト除外指定すると、
        // E を含む組み合わせは実質Lv6が3つに減るため、4つとも実質Lv6のABCDが優先されるはず。
        const A: i32 = 101;
        const B: i32 = 102;
        const C: i32 = 103;
        const D: i32 = 104;
        const E: i32 = 105;
        let modules = vec![
            module(1, 5500103, &[(A, 20)]),
            module(2, 5500103, &[(B, 20)]),
            module(3, 5500103, &[(C, 20)]),
            module(4, 5500103, &[(D, 20)]),
            module(5, 5500103, &[(E, 20)]),
        ];
        let res = optimize(&modules, &[], None, &[], &[E], &[], 5, 4, RankMode::Link)
            .expect("optimize should succeed");
        let best = &res.solutions[0];
        let best_keys: std::collections::BTreeSet<i64> =
            best.modules.iter().map(|m| m.key).collect();
        assert_eq!(
            best_keys,
            [1, 2, 3, 4].into_iter().collect(),
            "ABCD（4つとも実質Lv6）が最良のはず"
        );
        assert_eq!(best.lv6_count, 4);
        assert_eq!(best.selected_lv6, 0);
    }

    #[test]
    fn soft_exclude_breakdown_shows_excluded_attr_without_counting() {
        // A,B,C=Lv6, D,F=Lv5, E=Lv3（ソフト除外）が残る組み合わせを検証。
        // E は breakdown に現れ level=3・soft_excluded=true だが、lv6/lv5/eval_link
        // の集計には含まれない。link_effect（真値）には含まれる。
        const A: i32 = 101;
        const B: i32 = 102;
        const C: i32 = 103;
        const D: i32 = 104;
        const E: i32 = 105;
        const F: i32 = 106;
        let modules = vec![
            module(1, 5500103, &[(A, 20)]),
            module(2, 5500103, &[(B, 20)]),
            module(3, 5500103, &[(C, 20)]),
            module(4, 5500103, &[(D, 16)]),
            module(5, 5500103, &[(F, 16), (E, 8)]),
        ];
        let res = optimize(&modules, &[], None, &[], &[E], &[], 5, 5, RankMode::Link)
            .expect("optimize should succeed");
        let best = &res.solutions[0];
        assert_eq!(best.modules.len(), 5);
        assert_eq!(best.lv6_count, 3, "A,B,C の3つが実質Lv6");
        assert_eq!(best.lv5_count, 2, "D,F の2つが実質Lv5");
        assert_eq!(best.eval_link, 20 + 20 + 20 + 16 + 16);
        assert_eq!(
            best.link_effect,
            20 + 20 + 20 + 16 + 16 + 8,
            "真値はEの8も含む"
        );
        let e = best
            .breakdown
            .iter()
            .find(|b| b.attr_id == E)
            .expect("E は breakdown に含まれるはず");
        assert_eq!(e.level, 3);
        assert!(e.soft_excluded);
        assert_eq!(e.value, 8);
    }

    #[test]
    fn soft_exclude_excl_does_not_affect_counted_metrics() {
        // 「ソフト除外属性値をどのモジュールに足しても Key が増加しない（excl だけ悪化しうる）」
        // という単調性を検証する: counted 属性の寄与を揃えたまま soft-excluded 属性の値だけ
        // 増やしても、counted 側の指標（lv6/lv5/eval_link）は変化しない。
        const COUNTED: i32 = 201;
        const EXCL: i32 = 202;
        let filler = |k: i64| module(k, 5500103, &[(900 + k as i32, 1)]);
        let modules_low = vec![
            module(1, 5500103, &[(COUNTED, 10), (EXCL, 5)]),
            filler(2),
            filler(3),
            filler(4),
        ];
        let modules_high = vec![
            module(1, 5500103, &[(COUNTED, 10), (EXCL, 15)]),
            filler(2),
            filler(3),
            filler(4),
        ];
        let low = optimize(
            &modules_low,
            &[],
            None,
            &[],
            &[EXCL],
            &[],
            5,
            4,
            RankMode::Link,
        )
        .expect("optimize should succeed")
        .solutions
        .remove(0);
        let high = optimize(
            &modules_high,
            &[],
            None,
            &[],
            &[EXCL],
            &[],
            5,
            4,
            RankMode::Link,
        )
        .expect("optimize should succeed")
        .solutions
        .remove(0);
        assert_eq!(low.lv6_count, high.lv6_count);
        assert_eq!(low.lv5_count, high.lv5_count);
        assert_eq!(
            low.eval_link, high.eval_link,
            "counted 側は excl の値に左右されない"
        );
        assert_eq!(
            high.link_effect - low.link_effect,
            10,
            "真値の差は excl 分の差(15-5)のみ"
        );
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
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("読込失敗 {path}: {e}"));
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
            let _ = optimize(
                &modules,
                &[2104],
                Some("all"),
                &[],
                &[],
                &[],
                5,
                slot,
                RankMode::Link,
            );
            let t = Instant::now();
            let res = optimize(
                &modules,
                &[2104],
                Some("all"),
                &[],
                &[],
                &[],
                5,
                slot,
                RankMode::Link,
            )
            .expect("optimize should succeed");
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
    /// ソフト除外属性は counted 集計（lv6/lv5/eval_link）から除き、excl に加算する。
    /// requirements（属性ごとの下限Lv要求）を満たさない組み合わせは最適化版と同じく除外する。
    /// `mode` で成分並びを切り替える（[`Accum::key`] と同じ規則）。旧実装はタプルの型差
    /// （eval_link: i32 / lv5: usize）が「並び替えの反映漏れ」を検出するコンパイル時ガードを
    /// 兼ねていたが、`Key` の配列化でその保証が失われたため、両モードでこの参照実装との
    /// ブルートフォース突き合わせを必ず回す運用にする（呼び出し側テスト参照）。
    fn naive_ranked(
        modules: &[Module],
        selected_ids: &[i32],
        soft_exclude_ids: &[i32],
        requirements: &[(i32, usize)],
        slot_count: usize,
        top_k: usize,
        mode: RankMode,
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
            let meets_requirements = requirements.iter().all(|&(attr_id, lv)| {
                lv == 0 || level_of(*totals.get(&attr_id).unwrap_or(&0)) >= lv
            });
            if !meets_requirements {
                // 次の組み合わせへ進む処理を共有するため、下の advance ロジックへ流す。
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
                continue;
            }
            let (mut lv6, mut lv5, mut sel_lv6, mut sel_present, mut eval_link, mut excl) =
                (0, 0, 0, 0usize, 0i32, 0i32);
            for (&id, &v) in &totals {
                if soft_exclude_ids.contains(&id) {
                    excl += v;
                    continue;
                }
                let lv = level_of(v);
                eval_link += v;
                if lv == 6 {
                    lv6 += 1;
                } else if lv == 5 {
                    lv5 += 1;
                }
                if selected_ids.contains(&id) {
                    if lv == 6 {
                        sel_lv6 += 1;
                    }
                    // totals は combo に含まれる属性のみ（値≥1）なので lv>=1＝存在。
                    if lv >= 1 {
                        sel_present += 1;
                    }
                }
            }
            let neg_excl = -(excl as i64);
            let key: Key = match mode {
                RankMode::Link => [
                    sel_present as i64,
                    sel_lv6 as i64,
                    lv6 as i64,
                    eval_link as i64,
                    lv5 as i64,
                    neg_excl,
                ],
                RankMode::Lv5 => [
                    sel_present as i64,
                    sel_lv6 as i64,
                    lv6 as i64,
                    lv5 as i64,
                    eval_link as i64,
                    neg_excl,
                ],
            };
            out.push((key, combo.clone()));
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

    /// 最適化版（requirements 途中剪定・B&B 上界剪定ともに有効＝本番と同じ既定パイプライン）が
    /// 参照実装と完全一致（キー列・選択モジュール集合とも）することを検証。
    /// B&B は解を一切消さない設計（各上界は真の到達可能キー以上）のため、k-支配則と異なり
    /// これが割れることなく完全一致するはず＝歪みゼロの証明。
    /// [`RankMode::Link`]/[`RankMode::Lv5`] の両方で検証する（should_prune の並び替え対応が
    /// 両モードで健全であることの最重要ゲート）。
    #[test]
    fn matches_naive_reference() {
        let modules = synth_modules(28);
        let selected = [1003, 1007, 1011];
        let soft_exclude = [1002];
        for &mode in &[RankMode::Link, RankMode::Lv5] {
            for &slot in &[4usize, 5usize] {
                let top_k = 12;
                let res = optimize(
                    &modules,
                    &selected,
                    None,
                    &[],
                    &soft_exclude,
                    &[],
                    top_k,
                    slot,
                    mode,
                )
                .expect("optimize should succeed");
                let reference =
                    naive_ranked(&modules, &selected, &soft_exclude, &[], slot, top_k, mode);

                assert_eq!(
                    res.solutions.len(),
                    reference.len(),
                    "解の件数 mode={mode:?} slot={slot}"
                );
                assert_eq!(
                    res.combinations,
                    n_choose_k(modules.len(), slot),
                    "combinations は C(n,k) と一致すべき mode={mode:?} slot={slot}"
                );

                for (i, (sol, (key, combo))) in
                    res.solutions.iter().zip(reference.iter()).enumerate()
                {
                    assert_matches_reference(
                        sol,
                        key,
                        combo,
                        &modules,
                        mode,
                        &format!("mode={mode:?} slot={slot} rank={i}"),
                    );
                }
            }
        }
    }

    /// スレッド並列でも結果が決定的（2回実行で同一）であることを検証。
    #[test]
    fn deterministic_across_runs() {
        let modules = synth_modules(40);
        let a = optimize(
            &modules,
            &[1005],
            None,
            &[],
            &[],
            &[],
            10,
            5,
            RankMode::Link,
        )
        .expect("optimize should succeed");
        let b = optimize(
            &modules,
            &[1005],
            None,
            &[],
            &[],
            &[],
            10,
            5,
            RankMode::Link,
        )
        .expect("optimize should succeed");
        let keys = |r: &OptimizeResult| -> Vec<(i32, i32, Vec<i64>)> {
            r.solutions
                .iter()
                .map(|s| {
                    let mut ks: Vec<i64> = s.modules.iter().map(|m| m.key).collect();
                    ks.sort_unstable();
                    (s.eval_link, s.link_effect, ks)
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
        let res = optimize(&modules, &[1], None, &[], &[], &[], 5, 5, RankMode::Link)
            .expect("optimize should succeed");
        assert!(res.solutions.is_empty());
        assert_eq!(res.candidate_count, 4);
    }

    /// top-k のランキングキー列（sel_present..excl）を取り出す。同点タイの列挙メンバーが
    /// 変わりうる性能施策（requirements 途中剪定・k-支配則）の等価性検証に使う。
    fn key_seq(r: &OptimizeResult) -> Vec<(usize, usize, usize, i32, usize, i32)> {
        r.solutions
            .iter()
            .map(|s| {
                let excl: i32 = s
                    .breakdown
                    .iter()
                    .filter(|b| b.soft_excluded)
                    .map(|b| b.value)
                    .sum();
                (
                    s.selected_present,
                    s.selected_lv6,
                    s.lv6_count,
                    s.eval_link,
                    s.lv5_count,
                    excl,
                )
            })
            .collect()
    }

    /// requirements 途中剪定の on/off で top-k のキー列が一致すること（性能施策のみで
    /// 結果を変えないことの検証）。
    #[test]
    fn requirement_pruning_on_off_same_keys() {
        let modules = synth_modules(30);
        let selected = [1002, 1009];
        let requirements = [(1002, 4usize)];
        for &slot in &[4usize, 5usize] {
            let with_pruning = optimize_with_opts(
                &modules,
                &selected,
                None,
                &[],
                &[],
                &requirements,
                10,
                slot,
                RankMode::Link,
                true,
                false,
            )
            .expect("optimize should succeed");
            let without_pruning = optimize_with_opts(
                &modules,
                &selected,
                None,
                &[],
                &[],
                &requirements,
                10,
                slot,
                RankMode::Link,
                false,
                false,
            )
            .expect("optimize should succeed");
            assert_eq!(
                key_seq(&with_pruning),
                key_seq(&without_pruning),
                "requirements 途中剪定 on/off で slot={slot} のキー列が一致するはず"
            );
        }
    }

    /// B&B 上界剪定の on/off で top-k のキー列が一致すること（解を消さない性能施策のため、
    /// off と全く同じ結果になるはず）。高速な回帰チェック用（完全一致は下の全列挙比較で検証）。
    #[test]
    fn bnb_pruning_on_off_same_keys() {
        let modules = synth_modules(30);
        let selected = [1004];
        let soft_exclude = [1008];
        for &slot in &[4usize, 5usize] {
            let with_bnb = optimize_with_opts(
                &modules,
                &selected,
                None,
                &[],
                &soft_exclude,
                &[],
                10,
                slot,
                RankMode::Link,
                true,
                true,
            )
            .expect("optimize should succeed");
            let without_bnb = optimize_with_opts(
                &modules,
                &selected,
                None,
                &[],
                &soft_exclude,
                &[],
                10,
                slot,
                RankMode::Link,
                true,
                false,
            )
            .expect("optimize should succeed");
            assert_eq!(
                key_seq(&with_bnb),
                key_seq(&without_bnb),
                "B&B on/off で slot={slot} のキー列が一致するはず"
            );
        }
    }

    /// 参照実装との一致を検証する共通アサーション（キー各要素＋選択モジュール集合）。
    /// `mode` で Key の成分並びを切り替える（[`Accum::key`]/[`naive_ranked`] と同じ規則）。
    fn assert_matches_reference(
        sol: &Solution,
        key: &Key,
        combo: &[usize],
        modules: &[Module],
        mode: RankMode,
        ctx: &str,
    ) {
        assert_eq!(
            sol.selected_present as i64, key[0],
            "sel_present mismatch {ctx}"
        );
        assert_eq!(sol.selected_lv6 as i64, key[1], "sel_lv6 mismatch {ctx}");
        assert_eq!(sol.lv6_count as i64, key[2], "lv6 mismatch {ctx}");
        match mode {
            RankMode::Link => {
                assert_eq!(sol.eval_link as i64, key[3], "eval_link mismatch {ctx}");
                assert_eq!(sol.lv5_count as i64, key[4], "lv5 mismatch {ctx}");
            }
            RankMode::Lv5 => {
                assert_eq!(sol.lv5_count as i64, key[3], "lv5 mismatch {ctx}");
                assert_eq!(sol.eval_link as i64, key[4], "eval_link mismatch {ctx}");
            }
        }
        let excl: i32 = sol
            .breakdown
            .iter()
            .filter(|b| b.soft_excluded)
            .map(|b| b.value)
            .sum();
        assert_eq!(-(excl as i64), key[5], "excl mismatch {ctx}");
        let got: std::collections::BTreeSet<i64> = sol.modules.iter().map(|m| m.key).collect();
        let want: std::collections::BTreeSet<i64> = combo.iter().map(|&c| modules[c].key).collect();
        assert_eq!(got, want, "module set mismatch {ctx}");
    }

    /// 【最重要ゲート】B&B（分枝限定）は解を一切消さない設計であることを、多数のランダム構成
    /// （モジュール数・選択属性・ソフト除外属性・requirements・スロット数を総当たり）×
    /// 全列挙参照実装との完全一致（combo集合・キー列とも）で検証する。
    /// ここが割れたら `should_prune` の上界計算のどこかが過小評価（不健全）になっている
    /// ＝正当な解を刈ってしまっている（k-支配則と同じ轍）ということなので、絶対に緑であること。
    #[test]
    fn branch_and_bound_matches_naive_exhaustively() {
        let counts = [14usize, 18, 22, 26];
        let selected_sets: [&[i32]; 3] = [&[], &[1003], &[1002, 1009, 1013]];
        let soft_exclude_sets: [&[i32]; 3] = [&[], &[1005], &[1001, 1008]];
        let requirement_sets: [&[(i32, usize)]; 2] = [&[], &[(1002, 4)]];
        let top_k = 8;

        for &mode in &[RankMode::Link, RankMode::Lv5] {
            for &count in &counts {
                let modules = synth_modules(count);
                for &selected in &selected_sets {
                    for &soft_exclude in &soft_exclude_sets {
                        // 選択属性とソフト除外属性が重複する組み合わせは validate_inputs で
                        // エラーになる（意味のある指定ではない）ためスキップする。
                        if selected.iter().any(|id| soft_exclude.contains(id)) {
                            continue;
                        }
                        for &requirements in &requirement_sets {
                            if requirements
                                .iter()
                                .any(|&(id, lv)| lv > 0 && soft_exclude.contains(&id))
                            {
                                continue;
                            }
                            for &slot in &[4usize, 5usize] {
                                let res = optimize_with_opts(
                                    &modules,
                                    selected,
                                    None,
                                    &[],
                                    soft_exclude,
                                    requirements,
                                    top_k,
                                    slot,
                                    mode,
                                    true,
                                    true,
                                )
                                .unwrap_or_else(|e| panic!("optimize failed: {e}"));
                                let reference = naive_ranked(
                                    &modules,
                                    selected,
                                    soft_exclude,
                                    requirements,
                                    slot,
                                    top_k,
                                    mode,
                                );

                                let ctx = format!(
                                    "mode={mode:?} count={count} slot={slot} selected={selected:?} \
                                     soft_exclude={soft_exclude:?} requirements={requirements:?}"
                                );
                                assert_eq!(
                                    res.solutions.len(),
                                    reference.len(),
                                    "解の件数不一致 {ctx}"
                                );
                                for (i, (sol, (key, combo))) in
                                    res.solutions.iter().zip(reference.iter()).enumerate()
                                {
                                    assert_matches_reference(
                                        sol,
                                        key,
                                        combo,
                                        &modules,
                                        mode,
                                        &format!("{ctx} rank={i}"),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// rank-1（最良解）が、ソフト除外・選択属性・requirements を同時に使った場合でも
    /// 参照実装の top-1 と一致することを検証する。
    #[test]
    fn branch_and_bound_rank1_matches_naive_with_all_features() {
        let modules = synth_modules(24);
        let selected = [1004, 1011];
        let soft_exclude = [1002];
        let requirements = [(1004, 1usize)];
        for &mode in &[RankMode::Link, RankMode::Lv5] {
            for &slot in &[4usize, 5usize] {
                let res = optimize(
                    &modules,
                    &selected,
                    None,
                    &[],
                    &soft_exclude,
                    &requirements,
                    5,
                    slot,
                    mode,
                )
                .expect("optimize should succeed");
                let reference = naive_ranked(
                    &modules,
                    &selected,
                    &soft_exclude,
                    &requirements,
                    slot,
                    5,
                    mode,
                );
                assert!(
                    !res.solutions.is_empty(),
                    "解が存在するはず mode={mode:?} slot={slot}"
                );
                assert!(
                    !reference.is_empty(),
                    "参照実装にも解が存在するはず mode={mode:?} slot={slot}"
                );
                let (key, combo) = &reference[0];
                assert_matches_reference(
                    &res.solutions[0],
                    key,
                    combo,
                    &modules,
                    mode,
                    &format!("rank-1 mode={mode:?} slot={slot}"),
                );
            }
        }
    }

    /// attrs.rs の実属性IDを使った合成データ（決定的LCG、seed を変えて複数パターン生成）。
    /// 値1〜10、1モジュールあたり2〜3属性。属性IDは attrs.rs の実データから抜粋
    /// （Basic/Combat/Resist/Focus/Ultra の各グループを横断）。
    fn synth_modules_real_attrs(count: usize, seed: u64) -> Vec<Module> {
        // attrs.rs (ATTRS) より抜粋: 筋力強化/特攻ダメージ強化/魔法耐性/集中・会心/
        // 極・ダメージ増強/極・HP凝縮。
        const REAL_ATTR_IDS: [i32; 6] = [1110, 1113, 1307, 1409, 2104, 2204];
        let mut s = seed;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        (0..count)
            .map(|k| {
                let n_parts = 2 + (next() % 2) as usize; // 2 or 3
                let parts = (0..n_parts)
                    .map(|_| {
                        let attr_id = REAL_ATTR_IDS[(next() % REAL_ATTR_IDS.len() as u64) as usize];
                        let value = 1 + (next() % 10) as i32; // 1..=10
                        Part {
                            attr_id,
                            attr_name: crate::attrs::attr_name(attr_id).unwrap_or("?").to_string(),
                            value,
                        }
                    })
                    .collect();
                module_with_parts(k as i64, 5500103, parts)
            })
            .collect()
    }

    /// 【実データ寄り検証】属性IDを attrs.rs の実データから取り、値域(1〜10)・パーツ数(2〜3)も
    /// 実データに近づけた合成データで、複数 seed × n=18/22 × 選択属性/ソフト除外/requirements の
    /// 有無バリエーション × slot=4/5 を全数列挙参照実装と突き合わせる。
    /// branch_and_bound_matches_naive_exhaustively は合成属性ID(1000+)・値1〜9でより広く
    /// 総当たりしているが、こちらは実際の属性ID空間・値域・seedを変えても上界計算が
    /// 過小評価しない（＝解を刈らない）ことを別データ分布で追加確認する。
    #[test]
    fn branch_and_bound_matches_naive_with_real_attr_ids() {
        const SEL_A: i32 = 1113; // 特攻ダメージ強化
        const SEL_B: i32 = 2104; // 極・ダメージ増強
        const SEL_C: i32 = 1110; // 筋力強化
        const SOFT_EXCL: i32 = 1307; // 魔法耐性
        const REQ_ATTR: i32 = 1409; // 集中・会心

        // (選択属性, ソフト除外属性, requirements) の組み合わせ。
        // 「選択あり/なし」「ソフト除外あり」「requirements あり」の各単独バリエーションと、
        // それらを同時に使う組み合わせをカバーする。
        type Scenario<'a> = (&'a [i32], &'a [i32], &'a [(i32, usize)]);
        let scenarios: [Scenario; 5] = [
            (&[], &[], &[]),
            (&[SEL_A, SEL_B], &[], &[]),
            (&[], &[SOFT_EXCL], &[]),
            (&[], &[], &[(REQ_ATTR, 3)]),
            (&[SEL_C, SEL_B], &[SOFT_EXCL], &[(REQ_ATTR, 2)]),
        ];

        // seed を変えた複数ケース（属性値の分布が変わっても上界剪定が解を取りこぼさないことを見る）。
        let seeds: [u64; 3] = [
            0x1234_5678_9ABC_DEF0,
            0xDEAD_BEEF_CAFE_F00D,
            0x0BAD_C0DE_1337_BEEF,
        ];
        let counts = [18usize, 22]; // C(22,5)=26,334 まで（組合せ爆発しない範囲）。
        let top_k = 8;

        for &mode in &[RankMode::Link, RankMode::Lv5] {
            for &count in &counts {
                for &seed in &seeds {
                    let modules = synth_modules_real_attrs(count, seed);
                    for &(selected, soft_exclude, requirements) in &scenarios {
                        for &slot in &[4usize, 5usize] {
                            let res = optimize(
                                &modules,
                                selected,
                                None,
                                &[],
                                soft_exclude,
                                requirements,
                                top_k,
                                slot,
                                mode,
                            )
                            .unwrap_or_else(|e| panic!("optimize failed: {e}"));
                            let reference = naive_ranked(
                                &modules,
                                selected,
                                soft_exclude,
                                requirements,
                                slot,
                                top_k,
                                mode,
                            );

                            let ctx = format!(
                                "mode={mode:?} count={count} seed={seed:#x} slot={slot} \
                                 selected={selected:?} soft_exclude={soft_exclude:?} \
                                 requirements={requirements:?}"
                            );
                            assert_eq!(
                                res.solutions.len(),
                                reference.len(),
                                "解の件数不一致 {ctx}"
                            );
                            // TopK 全体のKey列（トップ1に限らず）を全て突き合わせる。search_cpu と
                            // naive_ranked は同じ goodness 降順・combo 昇順タイブレークで並べるため、
                            // 同点解でも順序を含めて一致するはず（順序差が出ればここで検出できる）。
                            for (i, (sol, (key, combo))) in
                                res.solutions.iter().zip(reference.iter()).enumerate()
                            {
                                assert_matches_reference(
                                    sol,
                                    key,
                                    combo,
                                    &modules,
                                    mode,
                                    &format!("{ctx} rank={i}"),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
