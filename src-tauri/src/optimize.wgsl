// GPU探索カーネル（feature "gpu"、optimizer_gpu.rs から include_str! で読み込む）。
//
// 方式: 全数評価 + CPUシード閾値によるフィルタリング（詳細は optimizer_gpu.rs のモジュール
// doc コメント参照）。
//
// スレッド割当（重要）: スレッド = 1組合せ ではなく、スレッド = (slot_count-1)個の
// 「プレフィックス」組合せ 1つ。各スレッドはプレフィックスを combinadic で unrank した後、
// 末尾候補 last を prefix_last+1..n でループしながら optimizer.rs の Accum::add/remove と
// 同じレベル遷移差分更新で6カウンタ（sel_present/sel_lv6/lv6/eval_link/lv5/excl）を
// 増分更新する。これにより「1組合せ=1スレッド」方式で全スレッドが O(n) の combinadic
// unrank（binomのグローバル読みが発散）を行っていたコストを、プレフィックスあたり1回
// （高々 slot_count-1 回の探索）に削減する。総プレフィックス数 P=C(n,slot_count-1) は
// n<=300 なら u32 に確実に収まるため、チャンク分割は不要（1ディスパッチで完結する）。
//
// プレフィックス単位枝刈り（Params.prune_enabled でON/OFF切替）: プレフィックスの6カウンタ
// 計算後・末尾ループ開始前に、optimizer.rs の should_prune の r=1（残り1枠のみ）相当の
// 上界キーを suffix_max テーブル（optimizer_gpu.rs の build_suffix_max_bytes、suffix の
// 単一最大値＝top-1）から計算し、CPUシード閾値（accept判定と同じ hi/lo 2値）を厳密に
// 下回るなら末尾ループを丸ごとスキップする。各成分は「suffix の単一最大値を独立に楽観視」
// した健全な上界（実到達可能キー以上であることが保証され、過小評価は絶対にしない）であり、
// 等号（上界==閾値）は絶対に刈らない（accept 判定と同じ半開区間: hi>threshold_hi ||
// (hi==threshold_hi && lo>=threshold_lo)）。
//
// レベル閾値・キー packing のビット割当・レベル遷移差分更新の規則は
// src-tauri/src/optimizer.rs の ATTR_THRESHOLDS / Accum::add・remove / Key と
// 完全に一致させること（変更時は両方同時に直す。ここがずれると等価性テストで検出される）。

const MAX_PARTS: u32 = 3u;
const MAX_ATTR: u32 = 32u;
const CAPACITY: u32 = 2097152u;
const NO_PART: u32 = 0xFFFFFFFFu;
// ワークグループ共有メモリへロードする候補データの上限件数。optimizer_gpu.rs の
// フォールバック判定（n>300 でCPUへ委譲）と一致させること。
const MAX_N: u32 = 300u;
const CAND_BUF_LEN: u32 = MAX_N * MAX_PARTS;
// プレフィックス長の上限（slot_count-1、slot_count<=5 なので最大4）。
const MAX_PREFIX: u32 = 4u;
// ランキング順序モード（optimizer_gpu.rs の rank_mode_u32 と一致させること）。
const RANK_MODE_LINK: u32 = 0u;
const RANK_MODE_LV5: u32 = 1u;

struct CandPart {
    attr_idx: u32,
    value: i32,
}

struct ReqEntry {
    attr_idx: u32,
    min_lv: u32,
}

// フィールド順は optimizer_gpu.rs の GpuParams（手動 to_le_bytes 直列化）と完全一致させること。
struct Params {
    n: u32,
    slot_count: u32,
    n_attr: u32,
    prefix_count: u32,
    wg_x: u32,
    table_cols: u32,
    selected_mask: u32,
    soft_excl_mask: u32,
    req_count: u32,
    threshold_hi: u32,
    threshold_lo: u32,
    prune_enabled: u32,
    rank_mode: u32,
}

@group(0) @binding(0) var<storage, read> binom: array<u32>;
@group(0) @binding(1) var<storage, read> cand_parts_g: array<CandPart>;
@group(0) @binding(2) var<storage, read> params: Params;
@group(0) @binding(3) var<storage, read> requirements: array<ReqEntry>;
@group(0) @binding(4) var<storage, read_write> out_combos: array<u32>;
// counters[0]=appended件数, counters[1]=pruned プレフィックス件数（枝刈りの効き計測用）。
@group(0) @binding(5) var<storage, read_write> counters: array<atomic<u32>, 2>;
// プレフィックス単位枝刈り用の suffix 最大値（r=1）テーブル。optimizer_gpu.rs の
// build_suffix_max_bytes と同じレイアウト:
// [attr_suffix_max: n_attr*(n+1) i32] ++ [w_suffix_max: (n+1) i32]
@group(0) @binding(6) var<storage, read> suffix_max: array<i32>;

// 候補データのワークグループ共有メモリキャッシュ（協調ロード。300*3*8B=7.2KB）。
// 末尾ループのグローバルメモリ読みをワークグループ内で共有し、メモリ律速を緩和する。
var<workgroup> cand_parts: array<CandPart, CAND_BUF_LEN>;

// optimizer.rs の ATTR_THRESHOLDS = [1, 4, 8, 12, 16, 20] を定数展開したもの。
fn level_of(v: i32) -> u32 {
    var lv: u32 = 0u;
    if (v >= 1) {
        lv = 1u;
    }
    if (v >= 4) {
        lv = 2u;
    }
    if (v >= 8) {
        lv = 3u;
    }
    if (v >= 12) {
        lv = 4u;
    }
    if (v >= 16) {
        lv = 5u;
    }
    if (v >= 20) {
        lv = 6u;
    }
    return lv;
}

fn binom_at(i: u32, j: u32) -> u32 {
    return binom[i * params.table_cols + j];
}

// suffix_max テーブルの各セクションへのアクセサ（optimizer_gpu.rs の build_suffix_max_bytes
// と同じレイアウト・同じ意味）。s は探索順 index（0..=n）。
fn attr_suffix_max_at(a: u32, s: u32) -> i32 {
    return suffix_max[a * (params.n + 1u) + s];
}
fn w_suffix_max_at(s: u32) -> i32 {
    return suffix_max[params.n_attr * (params.n + 1u) + s];
}

struct HiLo {
    hi: u32,
    lo: u32,
}

// キー packing（optimizer_gpu.rs の pack_key と同じビット割当）。RankMode ごとに
// 明示的な2分岐で実装する（汎用シフト/マスクの uniform 渡しは可読性が落ちてバグりやすく、
// ここは静かに順序が壊れると「刈りすぎ＝最適解を無言で失う」箇所のため、分岐コストより
// 可読性・レビュー容易性を優先する。スレッドあたり数回しか呼ばれないため分岐コストは
// 無視できる）。上界計算（ub_*）と最終キー計算の両方で共有する。
// - RANK_MODE_LINK: hi = sel_present(6)|sel_lv6(6)|lv6(6)|eval_link(14)、
//                   lo = lv5(16)|(0xFFFF-excl)(16)
// - RANK_MODE_LV5 : hi = sel_present(6)|sel_lv6(6)|lv6(6)|lv5(6)|未使用(8)、
//                   lo = eval_link(16)|(0xFFFF-excl)(16)
fn pack_hi_lo(sel_present: u32, sel_lv6: u32, lv6: u32, lv5: u32, eval_link: u32, excl: u32) -> HiLo {
    var result: HiLo;
    if (params.rank_mode == RANK_MODE_LV5) {
        result.hi = (sel_present << 26u) | (sel_lv6 << 20u) | (lv6 << 14u) | (lv5 << 8u);
        result.lo = (eval_link << 16u) | (0xFFFFu - excl);
    } else {
        result.hi = (sel_present << 26u) | (sel_lv6 << 20u) | (lv6 << 14u) | eval_link;
        result.lo = (lv5 << 16u) | (0xFFFFu - excl);
    }
    return result;
}

@compute @workgroup_size(256)
fn main(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_index) lidx: u32,
) {
    // --- 協調ロード: 候補データをワークグループ共有メモリへコピー。---
    let total_parts = params.n * MAX_PARTS;
    for (var i: u32 = lidx; i < total_parts; i = i + 256u) {
        cand_parts[i] = cand_parts_g[i];
    }
    workgroupBarrier();

    let group_linear = wg_id.y * params.wg_x + wg_id.x;
    let tid: u32 = group_linear * 256u + lidx;
    if (tid >= params.prefix_count) {
        return;
    }

    let k = params.slot_count;
    let kp = k - 1u; // プレフィックス長。

    // --- unrank: プレフィックス（kp個）を combinadic で展開（先頭候補=0からの全域探索）。---
    var prefix: array<u32, MAX_PREFIX>;
    var r: u32 = tid;
    var start: u32 = 0u;
    for (var pos: u32 = 0u; pos < kp; pos = pos + 1u) {
        let picks = kp - pos;
        var i: u32 = start;
        loop {
            let m = params.n - i - 1u;
            let gs = binom_at(m, picks - 1u);
            if (r < gs) {
                break;
            }
            r = r - gs;
            i = i + 1u;
        }
        prefix[pos] = i;
        start = i + 1u;
    }
    let prefix_last = prefix[kp - 1u];
    if (prefix_last + 1u >= params.n) {
        return; // 末尾候補が存在しない（このプレフィックスでは組合せを作れない）。
    }

    // --- プレフィックスの totals・6カウンタを一度だけフルスキャンで計算。---
    var totals: array<i32, MAX_ATTR>;
    for (var a: u32 = 0u; a < MAX_ATTR; a = a + 1u) {
        totals[a] = 0;
    }
    for (var pos: u32 = 0u; pos < kp; pos = pos + 1u) {
        let base = prefix[pos] * MAX_PARTS;
        for (var p: u32 = 0u; p < MAX_PARTS; p = p + 1u) {
            let part = cand_parts[base + p];
            if (part.attr_idx != NO_PART) {
                totals[part.attr_idx] = totals[part.attr_idx] + part.value;
            }
        }
    }

    var lv6: u32 = 0u;
    var lv5: u32 = 0u;
    var eval_link: i32 = 0;
    var excl: i32 = 0;
    var sel_lv6: u32 = 0u;
    var sel_present: u32 = 0u;
    for (var a: u32 = 0u; a < params.n_attr; a = a + 1u) {
        let v = totals[a];
        let bit = 1u << a;
        if ((params.soft_excl_mask & bit) != 0u) {
            excl = excl + v;
            continue;
        }
        let lv = level_of(v);
        eval_link = eval_link + v;
        if (lv == 6u) {
            lv6 = lv6 + 1u;
        } else if (lv == 5u) {
            lv5 = lv5 + 1u;
        }
        if ((params.selected_mask & bit) != 0u) {
            if (lv == 6u) {
                sel_lv6 = sel_lv6 + 1u;
            }
            if (lv >= 1u) {
                sel_present = sel_present + 1u;
            }
        }
    }

    // --- プレフィックス単位枝刈り（r=1: 残り1枠を最も都合よく埋めた場合の健全な上界）。---
    if (params.prune_enabled != 0u) {
        let ub_start = prefix_last + 1u;
        var ub_sel_present = sel_present;
        var ub_sel_lv6 = sel_lv6;
        var ub_lv6 = lv6;
        var ub_lv5 = lv5;
        for (var a: u32 = 0u; a < params.n_attr; a = a + 1u) {
            let bit = 1u << a;
            if ((params.soft_excl_mask & bit) != 0u) {
                continue;
            }
            let cur = totals[a];
            let cur_lv = level_of(cur);
            let best_add = attr_suffix_max_at(a, ub_start);
            let reach_lv = level_of(cur + best_add);
            if ((params.selected_mask & bit) != 0u) {
                if (cur_lv == 0u && reach_lv >= 1u) {
                    ub_sel_present = ub_sel_present + 1u;
                }
                if (cur_lv < 6u && reach_lv >= 6u) {
                    ub_sel_lv6 = ub_sel_lv6 + 1u;
                }
            }
            if (cur_lv < 6u && reach_lv >= 6u) {
                ub_lv6 = ub_lv6 + 1u;
            }
            if (cur_lv <= 4u && reach_lv >= 5u) {
                ub_lv5 = ub_lv5 + 1u;
            }
        }
        let ub_eval_link = eval_link + w_suffix_max_at(ub_start);
        let ub_hilo = pack_hi_lo(ub_sel_present, ub_sel_lv6, ub_lv6, ub_lv5, u32(ub_eval_link), u32(excl));
        let ub_hi = ub_hilo.hi;
        let ub_lo = ub_hilo.lo;
        // accept 判定と同じ半開区間セマンティクス。上界が閾値と同点（==）の場合は accept 側に
        // 倒れる（!ub_accept は false）ため、絶対に刈らない。
        let ub_accept = (ub_hi > params.threshold_hi)
            || (ub_hi == params.threshold_hi && ub_lo >= params.threshold_lo);
        if (!ub_accept) {
            atomicAdd(&counters[1], 1u);
            return;
        }
    }

    // --- 末尾候補ループ: optimizer.rs の Accum::add/remove と同じ差分更新で6カウンタを
    //     増分更新する（値は非負なので add では new_lv>=old_lv、remove では
    //     cur_lv>=new_lv が常に成り立ち、u32減算がアンダーフローすることはない）。---
    for (var last: u32 = prefix_last + 1u; last < params.n; last = last + 1u) {
        let base = last * MAX_PARTS;

        // add
        for (var p: u32 = 0u; p < MAX_PARTS; p = p + 1u) {
            let part = cand_parts[base + p];
            if (part.attr_idx == NO_PART) {
                continue;
            }
            let a = part.attr_idx;
            let bit = 1u << a;
            if ((params.soft_excl_mask & bit) != 0u) {
                excl = excl + part.value;
                continue;
            }
            let old = totals[a];
            let newv = old + part.value;
            let old_lv = level_of(old);
            let new_lv = level_of(newv);
            if (new_lv != old_lv) {
                if (old_lv == 6u) {
                    lv6 = lv6 - 1u;
                }
                if (old_lv == 5u) {
                    lv5 = lv5 - 1u;
                }
                if (new_lv == 6u) {
                    lv6 = lv6 + 1u;
                }
                if (new_lv == 5u) {
                    lv5 = lv5 + 1u;
                }
                if ((params.selected_mask & bit) != 0u) {
                    if (old_lv == 6u) {
                        sel_lv6 = sel_lv6 - 1u;
                    }
                    if (new_lv == 6u) {
                        sel_lv6 = sel_lv6 + 1u;
                    }
                    if (old_lv == 0u) {
                        sel_present = sel_present + 1u;
                    }
                }
            }
            totals[a] = newv;
            eval_link = eval_link + part.value;
        }

        // --- requirements（属性ごとの下限Lv要求）判定。---
        var reqs_ok = true;
        for (var ri: u32 = 0u; ri < params.req_count; ri = ri + 1u) {
            let req = requirements[ri];
            if (level_of(totals[req.attr_idx]) < req.min_lv) {
                reqs_ok = false;
                break;
            }
        }

        if (reqs_ok) {
            // --- キー packing・閾値比較。---
            let hilo = pack_hi_lo(sel_present, sel_lv6, lv6, lv5, u32(eval_link), u32(excl));
            let hi = hilo.hi;
            let lo = hilo.lo;
            let accept = (hi > params.threshold_hi)
                || (hi == params.threshold_hi && lo >= params.threshold_lo);
            if (accept) {
                let out_idx = atomicAdd(&counters[0], 1u);
                if (out_idx < CAPACITY) {
                    let obase = out_idx * 5u;
                    for (var pos: u32 = 0u; pos < kp; pos = pos + 1u) {
                        out_combos[obase + pos] = prefix[pos];
                    }
                    out_combos[obase + kp] = last;
                    for (var pos: u32 = k; pos < 5u; pos = pos + 1u) {
                        out_combos[obase + pos] = NO_PART;
                    }
                }
            }
        }

        // remove（差分を巻き戻し、次の last へ備える）
        for (var p: u32 = 0u; p < MAX_PARTS; p = p + 1u) {
            let part = cand_parts[base + p];
            if (part.attr_idx == NO_PART) {
                continue;
            }
            let a = part.attr_idx;
            let bit = 1u << a;
            if ((params.soft_excl_mask & bit) != 0u) {
                excl = excl - part.value;
                continue;
            }
            let cur = totals[a];
            let newv = cur - part.value;
            let cur_lv = level_of(cur);
            let new_lv = level_of(newv);
            if (cur_lv != new_lv) {
                if (cur_lv == 6u) {
                    lv6 = lv6 - 1u;
                }
                if (cur_lv == 5u) {
                    lv5 = lv5 - 1u;
                }
                if (new_lv == 6u) {
                    lv6 = lv6 + 1u;
                }
                if (new_lv == 5u) {
                    lv5 = lv5 + 1u;
                }
                if ((params.selected_mask & bit) != 0u) {
                    if (cur_lv == 6u) {
                        sel_lv6 = sel_lv6 - 1u;
                    }
                    if (new_lv == 6u) {
                        sel_lv6 = sel_lv6 + 1u;
                    }
                    if (new_lv == 0u) {
                        sel_present = sel_present - 1u;
                    }
                }
            }
            totals[a] = newv;
            eval_link = eval_link - part.value;
        }
    }
}
