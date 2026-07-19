// GPU探索カーネル（2フェーズ版、Phase B2）。feature "gpu" 専用ビルド、optimizer_gpu.rs から
// include_str! で読み込む。
//
// Phase B（単一パス+プレフィックス単位枝刈り、optimize.wgsl）は、ワープ内に1本でも
// 「生存して末尾ループを最後まで実行するスレッド」が残ると、そのワープ全体の完了時間が
// 縮まらないSIMT特性の限界に直面した（実測: n=300 slot5、枝刈り率93%でも高速化はほぼゼロ。
// 素の単一パスカーネルとの比較で確認済み）。このファイルは、生存者だけを別バッファへ
// 密に集めてから本処理する stream compaction 方式（2カーネル分離）で、この限界を回避する。
//
// 1. Kernel P（プレフィルタ、main_p）: 1スレッド=1プレフィックスrank。
//    unrank→プレフィックスaccum→requirements実現可能性チェック→admissible上界計算
//    （optimize.wgsl のPhase B枝刈りと同じテーブル・同じ等号セマンティクス: 上界=閾値は
//    絶対に刈らない）。生存（requirements実現可能かつ上界が閾値以上）なら survivors[] へ
//    global な prefix rank を atomicAdd 経由で密に書く。append処理・末尾ループは一切
//    含まない（レジスタ最小に保つのが目的）。requirements実現可能性チェック（r=1相当。
//    attr_suffix_max の suffix 単一最大値を使い「残り1枠でも required 属性の下限Lvへ
//    絶対に届かない」プレフィックスを即座に打ち切る）はB&B上界と独立した必要十分条件
//    （過大評価も過小評価もしない）で admissible。requirements 条件下で B&B 上界だけでは
//    枝刈り率が伸びず Kernel T（末尾ループ）側の負荷が支配的になる問題（実測: n=300
//    slot5、B&B上界のみだと枝刈り率72〜76%・requirements無しの93〜96%より大幅に低い）
//    に対処する。
// 2. Kernel I（indirect dispatch args builder、main_i）: 1スレッドのみ。survivor数から
//    Kernel T のワークグループ数を計算し、indirect dispatch 用の領域（counters[3..6]）へ書く。
// 3. Kernel T（本体、main_t）: optimize.wgsl の単一パスカーネルから枝刈り分岐を除いた
//    「素の」ロジックと完全に同一（プレフィックス単位枝刈りを一切持たずレジスタ圧迫を避ける。
//    2.50s の素の性能プロファイルを維持することが目的）。thread id を直接 prefix rank に
//    する代わりに survivors[thread id] を prefix rank として読む点のみが異なる。
//
// ホスト側（optimizer_gpu.rs の run_gpu_search_chunked）は、n<=300 全域を固定サイズ
// チャンク（CHUNK_SIZE=8M prefix、workgroup 256 換算で 32768 workgroups）に分割し、
// チャンク毎に P→I→T を実行する（indirect dispatch によりチャンク間のCPU読み戻しは
// 発生しない。全チャンク投入後に1回だけ結果を読み戻す）。survivors バッファはチャンク
// サイズ分だけ確保して使い回す（生存者数はチャンク内プレフィックス数を超えないため
// オーバーフローが構造的に起きない）。
//
// レベル閾値・キー packing のビット割当・レベル遷移差分更新の規則は optimize.wgsl と
// 完全に一致させること（変更時は両方同時に直す。ここがずれると等価性テストで検出される）。

const MAX_PARTS: u32 = 3u;
const MAX_ATTR: u32 = 32u;
const CAPACITY: u32 = 2097152u;
const NO_PART: u32 = 0xFFFFFFFFu;
// ワークグループ共有メモリへロードする候補データの上限件数。optimizer_gpu.rs の
// フォールバック判定（n>300 でCPUへ委譲）・optimize.wgsl の MAX_N と一致させること。
const MAX_N: u32 = 300u;
const CAND_BUF_LEN: u32 = MAX_N * MAX_PARTS;
// プレフィックス長の上限（slot_count-1、slot_count<=5 なので最大4）。
const MAX_PREFIX: u32 = 4u;

struct CandPart {
    attr_idx: u32,
    value: i32,
}

struct ReqEntry {
    attr_idx: u32,
    min_lv: u32,
}

// フィールド順は optimizer_gpu.rs の build_params_chunked_bytes と完全一致させること。
// chunk_start/chunk_count はチャンク毎に書き換えて使い回す（他フィールドは全チャンクで不変）。
struct Params {
    n: u32,
    slot_count: u32,
    n_attr: u32,
    table_cols: u32,
    selected_mask: u32,
    soft_excl_mask: u32,
    req_count: u32,
    threshold_hi: u32,
    threshold_lo: u32,
    chunk_start: u32,
    chunk_count: u32,
}

@group(0) @binding(0) var<storage, read> binom: array<u32>;
@group(0) @binding(1) var<storage, read> cand_parts_g: array<CandPart>;
@group(0) @binding(2) var<storage, read> params: Params;
@group(0) @binding(3) var<storage, read> requirements: array<ReqEntry>;
@group(0) @binding(4) var<storage, read> suffix_max: array<i32>;
// survivors[i] = i番目に生存したプレフィックスのglobal rank（Kernel Pが書き、Kernel Tが読む）。
// チャンクサイズ分の容量を確保し全チャンクで使い回す（各チャンクの Kernel P 呼び出しで
// 実質上書きされる。生存者数 <= チャンク内プレフィックス数のためオーバーフローしない）。
@group(0) @binding(5) var<storage, read_write> survivors: array<u32>;
// counters[0]=appended(全チャンク累積), counters[1]=survivor_total(全チャンク累積・診断用),
// counters[2]=chunk_survivor_count(チャンク毎にホスト側で0リセット、Kernel I/Tが読む)。
@group(0) @binding(6) var<storage, read_write> counters: array<atomic<u32>, 3>;
@group(0) @binding(7) var<storage, read_write> out_combos: array<u32>;
// indirect dispatch args(x,y,z)。Kernel I が書き、CPU側の dispatch_workgroups_indirect が
// 直接読む。counters と同一バッファに同居させると、Kernel T の dispatch コマンド内で
// 「shaderによる storage read_write」と「同一コマンドの indirect args 読み出し」が競合し
// wgpu のバリデーションで拒否される（バイトオフセットが重ならなくてもバッファ単位で
// 排他扱いになる）ため、独立した専用バッファ・専用バインディングにする必要がある
// （Kernel T の bind group には含めない。main_i でのみ使用）。
@group(0) @binding(8) var<storage, read_write> indirect_args: array<atomic<u32>, 3>;

// 候補データのワークグループ共有メモリキャッシュ（協調ロード）。main_p/main_t の両方で
// 使い回す（ディスパッチが異なれば内容も独立するため競合しない。変数を分けて二重確保
// しないことで共有メモリ使用量を確実に単一カーネル分（300*3*8B=7.2KB）に抑える）。
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

// optimizer.rs の ATTR_THRESHOLDS[lv-1]（lv=1..6 到達に必要な最小値）と同じ。
// Kernel P の requirements 実現可能性チェック専用。
fn threshold_for_level(lv: u32) -> i32 {
    if (lv <= 1u) {
        return 1;
    }
    if (lv == 2u) {
        return 4;
    }
    if (lv == 3u) {
        return 8;
    }
    if (lv == 4u) {
        return 12;
    }
    if (lv == 5u) {
        return 16;
    }
    return 20; // lv >= 6
}

fn binom_at(i: u32, j: u32) -> u32 {
    return binom[i * params.table_cols + j];
}

// suffix_max テーブルの各セクションへのアクセサ（optimizer_gpu.rs の build_suffix_max_bytes
// と同じレイアウト・同じ意味。Kernel P の上界計算専用）。
fn attr_suffix_max_at(a: u32, s: u32) -> i32 {
    return suffix_max[a * (params.n + 1u) + s];
}
fn w_suffix_max_at(s: u32) -> i32 {
    return suffix_max[params.n_attr * (params.n + 1u) + s];
}
fn g_suffix_max_at(s: u32) -> i32 {
    return suffix_max[params.n_attr * (params.n + 1u) + (params.n + 1u) + s];
}

// --- Kernel P: プレフィルタ。1スレッド=1プレフィックスrank（チャンクローカル）。 ---
@compute @workgroup_size(256)
fn main_p(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_index) lidx: u32,
) {
    let total_parts = params.n * MAX_PARTS;
    for (var i: u32 = lidx; i < total_parts; i = i + 256u) {
        cand_parts[i] = cand_parts_g[i];
    }
    workgroupBarrier();

    let local_tid = wg_id.x * 256u + lidx;
    if (local_tid >= params.chunk_count) {
        return;
    }
    let global_rank = params.chunk_start + local_tid;

    let k = params.slot_count;
    let kp = k - 1u;

    // --- unrank: プレフィックス（kp個）を combinadic で展開。 ---
    var prefix: array<u32, MAX_PREFIX>;
    var r: u32 = global_rank;
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
    let ub_start = prefix_last + 1u;

    // --- プレフィックスの totals・7カウンタを一度だけフルスキャンで計算。 ---
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

    // --- requirements 実現可能性チェック（r=1: 残り1枠でも必要な下限Lvへ絶対に届かない
    //     required 属性が1つでもあれば、このプレフィックスをここで打ち切る）。
    //     attr_suffix_max は「残り候補群から選べる単一候補の最大寄与」の厳密な最大値
    //     （suffix top-1）であり、totals[attr]+best_add はその属性が残り1枠で到達しうる
    //     真の最大値そのもの（過大評価でも過小評価でもない）。したがって、この判定は
    //     「どの候補を最後に選んでも当該属性の要求Lvへ絶対に届かない」ことの必要十分条件
    //     であり、admissible（取りこぼしゼロ）を厳密に満たす。requirements は
    //     validate_inputs（optimizer.rs）でソフト/ハード除外属性との重複が既に排除されて
    //     いるため soft_excl_mask によるフィルタは不要。
    for (var ri: u32 = 0u; ri < params.req_count; ri = ri + 1u) {
        let req = requirements[ri];
        let needed = threshold_for_level(req.min_lv);
        let best_add = attr_suffix_max_at(req.attr_idx, ub_start);
        if (totals[req.attr_idx] + best_add < needed) {
            return; // 要求属性の1つが絶対に満たせない。生存させない。
        }
    }

    var lv6: u32 = 0u;
    var lv5: u32 = 0u;
    var level_sum: u32 = 0u;
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
        level_sum = level_sum + lv;
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

    // --- admissible 上界（r=1: 残り1枠を最も都合よく埋めた場合）。 ---
    // optimizer.rs の should_prune の r=1 相当。各成分は「suffix の単一最大値を独立に
    // 楽観視」した健全な上界（過小評価は絶対にしない）。ub_start は requirements
    // 実現可能性チェックで既に計算済みのものを使い回す。
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
    let ub_level_sum = level_sum + u32(g_suffix_max_at(ub_start));
    let ub_eval_link = eval_link + w_suffix_max_at(ub_start);
    let ub_hi = (ub_sel_present << 26u) | (ub_sel_lv6 << 20u) | (ub_lv6 << 14u) | (ub_lv5 << 8u) | ub_level_sum;
    let ub_lo = (u32(ub_eval_link) << 16u) | (0xFFFFu - u32(excl));
    // accept 判定（Kernel T）と同じ半開区間セマンティクス。上界が閾値と同点（==）の場合は
    // accept 側に倒れる（!ub_accept は false）ため、絶対に刈らない。
    let ub_accept = (ub_hi > params.threshold_hi)
        || (ub_hi == params.threshold_hi && ub_lo >= params.threshold_lo);
    if (!ub_accept) {
        return; // 生存しない: 末尾ループ相当の処理を一切行わずスレッド終了。
    }

    atomicAdd(&counters[1], 1u); // 診断用: 全チャンク累積の生存数。
    let chunk_idx = atomicAdd(&counters[2], 1u); // チャンクローカルの生存インデックス。
    survivors[chunk_idx] = global_rank;
}

// --- Kernel I: indirect dispatch args builder。1スレッドのみで足りる。 ---
@compute @workgroup_size(1)
fn main_i() {
    let survivor_count = atomicLoad(&counters[2]);
    let wg_needed = (survivor_count + 255u) / 256u;
    atomicStore(&indirect_args[0], wg_needed);
    atomicStore(&indirect_args[1], 1u);
    atomicStore(&indirect_args[2], 1u);
}

// --- Kernel T: 本体。optimize.wgsl の単一パスカーネルから枝刈り分岐を除いた「素の」
//     ロジックと完全に同一（レジスタ圧迫を避けるため上界計算・フラグ分岐を一切持たない）。
//     tid を直接 prefix rank にする代わりに survivors[tid] を prefix rank として読む点のみ
//     が異なる。 ---
@compute @workgroup_size(256)
fn main_t(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_index) lidx: u32,
) {
    let total_parts = params.n * MAX_PARTS;
    for (var i: u32 = lidx; i < total_parts; i = i + 256u) {
        cand_parts[i] = cand_parts_g[i];
    }
    workgroupBarrier();

    let tid: u32 = wg_id.x * 256u + lidx;
    let survivor_count = atomicLoad(&counters[2]);
    if (tid >= survivor_count) {
        return;
    }
    let global_rank = survivors[tid];

    let k = params.slot_count;
    let kp = k - 1u; // プレフィックス長。

    // --- unrank: Kernel P と全く同じロジック（global_rank から再展開）。 ---
    var prefix: array<u32, MAX_PREFIX>;
    var r: u32 = global_rank;
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
        // Kernel P の生存判定を通過したプレフィックスのみが survivors に入るため理論上
        // 到達しないが、素のカーネルと同じ防御チェックとして残す。
        return;
    }

    // --- プレフィックスの totals・7カウンタを一度だけフルスキャンで計算。 ---
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
    var level_sum: u32 = 0u;
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
        level_sum = level_sum + lv;
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

    // --- 末尾候補ループ: optimizer.rs の Accum::add/remove と同じ差分更新で7カウンタを
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
                level_sum = level_sum + (new_lv - old_lv);
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

        // --- requirements（属性ごとの下限Lv要求）判定。 ---
        var reqs_ok = true;
        for (var ri: u32 = 0u; ri < params.req_count; ri = ri + 1u) {
            let req = requirements[ri];
            if (level_of(totals[req.attr_idx]) < req.min_lv) {
                reqs_ok = false;
                break;
            }
        }

        if (reqs_ok) {
            // --- キー packing（optimizer_gpu.rs の pack_key と同じビット割当）・閾値比較。 ---
            let hi = (sel_present << 26u) | (sel_lv6 << 20u) | (lv6 << 14u) | (lv5 << 8u) | level_sum;
            let lo = (u32(eval_link) << 16u) | (0xFFFFu - u32(excl));
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
                level_sum = level_sum - (cur_lv - new_lv);
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
