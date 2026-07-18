//! GPU探索（feature "gpu" 専用ビルド）。
//!
//! 方式: 全数評価 + CPUシード閾値 + append + CPU厳密再計算マージ。
//! 再帰DFS + データ依存B&B（[`crate::optimizer`] の CPU 探索）は SIMT 非対応のため、
//! GPU では全組合せを力任せに評価する。ただし「閾値未満は書き出さない」フィルタで出力を
//! 絞り、正確性は CPU 側の厳密再計算で担保する（GPU 側の結果は一切信用せず、キーは常に
//! [`crate::optimizer::Accum`] で再計算する）。
//!
//! 1. **prep（CPU・[`crate::optimizer::prepare`] を共有）**: 候補フィルタ→属性密インデックス化
//!    →w(m)降順ソート。
//! 2. **シード（CPU）**: 「w(m)降順上位 min(n,60) 件」∪「各 requirements 属性ごとに、その
//!    属性の合計値が大きい順に上位10件」の和集合（[`build_seed_positions`]）で既存 B&B DFS
//!    （[`crate::optimizer::search_cpu`]）→厳密な暫定 top-k。その k位キーを閾値とする
//!    （真のk位キーの下界なので、閾値以上を全部拾えば真のtop-kを取りこぼさない。部分集合の
//!    top-kは全候補でのtop-k以下という単調性による）。w降順上位だけでは
//!    requirements を満たす解が top_k 件見つからず閾値が効かない（append 爆発）ケースを
//!    緩和するため、requirements 属性ごとの上位も足す。
//! 3. **GPUカーネル**（`optimize.wgsl`、workgroup 256、2Dディスパッチ、単一ディスパッチ）:
//!    スレッド = 1組合せ ではなく、スレッド = (slot_count-1)個の「プレフィックス」組合せ
//!    1つ。プレフィックスを combinadic で unrank した後、末尾候補を prefix_last+1..n で
//!    ループしながら [`crate::optimizer::Accum::add`]/`remove` と同じレベル遷移差分更新で
//!    7カウンタを増分更新する（「1組合せ=1スレッド」方式で全スレッドが O(n) の unrank を
//!    行っていたコストを、プレフィックスあたり1回に削減）。候補データはワークグループ共有
//!    メモリへ協調ロードする。総プレフィックス数 P=C(n,slot_count-1) は n<=300 なら u32 に
//!    確実に収まるため、チャンク分割は不要（1ディスパッチで完結する）。
//! 4. **マージ（CPU）**: append された combo を厳密再計算（[`crate::optimizer::Accum`]）→
//!    [`crate::optimizer::TopK`] へマージ→top-k確定→[`crate::optimizer::assemble`]。
//!    最終順序はCPU厳密計算のみで決まるため、CPU版と完全一致する。
//! 5. append バッファがあふれたら（counter>容量）そのままクエリ全体を CPU 探索へ
//!    フォールバックする（再試行はしない）。atomicAdd の到達順は実行のたびに変わりうる
//!    任意サブセットのため、部分出力から閾値を引き締めて再試行するのは不健全
//!    （真の解を取りこぼしうる）。
//!
//! フォールバック条件（すべて log::warn した上で [`crate::optimizer::optimize`] へ委譲。
//! どんな失敗でもユーザーにエラーを返さず CPU で完遂する）:
//! - GPU adapter/device 初期化失敗
//! - n_attr > 32（キー packing・ビットマスクが u32 に収まらない）
//! - top_k > 64
//! - slot_count が 4/5 以外
//! - n > [`MAX_N`]=300（プレフィックスを共有メモリへ協調ロードする候補データ配列の
//!   固定長上限。optimize.wgsl の `MAX_N` と一致させること）
//! - モジュールのパーツ数が GPU カーネルの前提（[`MAX_PARTS`]=3）を超える
//!   （実データ・design docの前提は3だが、将来データが崩れた場合の安全弁として追加）
//! - eval_link・excl の理論上界が 0xFFFF を超える（キー packing の bit 幅を超える）
//! - GPU 実行時エラー・panic（デバイスロスト、シェーダ実行時エラー等）

use crate::optimizer::{self, Accum, Cand, Key, Module, OptimizeResult, Prepared, Ranked, TopK};
use std::cmp::Reverse;
use std::sync::OnceLock;
use wgpu::util::DeviceExt;

/// GPUカーネルが前提とするモジュール1件あたりの最大パーツ数（optimize.wgsl の
/// `MAX_PARTS` と一致させること）。実データ・合成データとも現状は最大3だが、
/// 将来のゲームデータ変更で超過した場合は静かに切り捨てず CPU へフォールバックする。
const MAX_PARTS: usize = 3;

/// append バッファの容量（optimize.wgsl の `CAPACITY` と一致させること）。
const CAPACITY: u32 = 2_097_152;

/// GPUカーネルが候補データをワークグループ共有メモリへ協調ロードする際の固定長上限
/// （optimize.wgsl の `MAX_N` と一致させること）。n がこれを超えたらCPUへフォールバックする。
/// n<=300 なら slot5 のプレフィックス数 C(300,4)≈3.3億 も u32 に確実に収まる。
const MAX_N: usize = 300;

/// CPUシードに requirements 属性ごとの上位を追加する件数（`build_seed_positions` 参照）。
const SEED_TOP_PER_REQUIRED_ATTR: usize = 10;

/// CPUシードの w(m) 降順上位件数（`build_seed_positions` 参照）。
const SEED_TOP_BY_WEIGHT: usize = 60;

struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

static GPU_CONTEXT: OnceLock<Result<GpuContext, String>> = OnceLock::new();

/// panic payload から可能な限り人間可読なメッセージを取り出す（`&str`/`String` 以外は
/// 定型メッセージにフォールバック）。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "詳細不明のpanic".to_string()
    }
}

/// GPUコンテキストを取得する（初回のみ初期化し、以後は `OnceLock` でキャッシュ）。
/// `init_gpu_context` は `create_shader_module`/`create_compute_pipeline` など Result を
/// 返さない wgpu API を呼ぶため、ドライバ側のバリデーション/コンパイル失敗が
/// panic（uncaptured error handler 経由）として現れることがある。ここで捕捉せず
/// `spawn_blocking` を突き抜けさせると「どんな失敗でもCPUで完遂する」という設計原則が
/// 破れるため、初期化全体を `catch_unwind` で包み、panic も `Err` として OnceLock に
/// キャッシュする（以後のクエリは初期化を再試行せず、毎回 warn ログ+CPUフォールバックに
/// 固定される）。
fn gpu_context() -> Result<&'static GpuContext, String> {
    GPU_CONTEXT
        .get_or_init(|| {
            std::panic::catch_unwind(init_gpu_context).unwrap_or_else(|payload| {
                Err(format!(
                    "GPU初期化中にpanicが発生しました: {}",
                    panic_message(payload.as_ref())
                ))
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// GPUコンテキストの事前初期化（デバイス取得+パイプラインコンパイル）。実測で初回クエリのみ
/// +0.2〜1.5s 乗るため、アプリ起動時にバックグラウンドスレッドから叩いて先食いする用途。
/// 探索クエリと同じ [`gpu_context`]（`OnceLock`）を叩くだけで、結果はそのままキャッシュに
/// 乗る（失敗しても後続のクエリが通常どおりCPUへフォールバックするだけなので、戻り値は
/// ログ出力のみに使う想定）。
pub fn prewarm() {
    match gpu_context() {
        Ok(_) => log::info!("[gpu] プリウォーム完了（デバイス初期化・パイプライン構築済み）"),
        Err(e) => log::warn!("[gpu] プリウォーム失敗（クエリ時にCPUへフォールバックします）: {e}"),
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn init_gpu_context() -> Result<GpuContext, String> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .map_err(|e| format!("GPUアダプタ取得失敗: {e}"))?;

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("bpsr-module-optimizer-gpu"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    }))
    .map_err(|e| format!("GPUデバイス取得失敗: {e}"))?;

    // create_shader_module/create_compute_pipeline は Result を返さない wgpu API のため、
    // シェーダのバリデーション失敗やドライバ側コンパイル失敗は既定で uncaptured error
    // handler（panic）に流れる。エラースコープで囲み、Result として拾えるようにする
    // （古いドライバ・非対応機能等での初期化失敗を panic ではなく Err にする）。
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("optimize-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("optimize.wgsl").into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("optimize-bgl"),
        entries: &[
            storage_entry(0, true),
            storage_entry(1, true),
            storage_entry(2, true),
            storage_entry(3, true),
            storage_entry(4, false),
            storage_entry(5, false),
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("optimize-pl"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("optimize-pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });

    if let Some(e) = pollster::block_on(error_scope.pop()) {
        return Err(format!("シェーダ/パイプライン構築に失敗しました: {e}"));
    }

    Ok(GpuContext {
        device,
        queue,
        pipeline,
        bind_group_layout,
    })
}

/// キー（[`Key`]）を optimize.wgsl の hi/lo packing と同じビット割当で u32 2つへ詰める。
/// hi: sel_present(6bit) | sel_lv6(6bit) | lv6(6bit) | lv5(6bit) | level_sum(8bit)
/// lo: eval_link(16bit) | (0xFFFF - excl)(16bit)
/// 各成分が対応 bit 幅に収まることは呼び出し側のフォールバック判定（n_attr<=32・
/// eval_link/excl上界<=0xFFFF）で事前に保証されている。
fn pack_key(key: &Key) -> (u32, u32) {
    let (sel_present, sel_lv6, lv6, lv5, level_sum, eval_link, Reverse(excl)) = *key;
    let hi = ((sel_present as u32) << 26)
        | ((sel_lv6 as u32) << 20)
        | ((lv6 as u32) << 14)
        | ((lv5 as u32) << 8)
        | (level_sum as u32);
    let lo = ((eval_link as u32) << 16) | (0xFFFFu32 - (excl as u32));
    (hi, lo)
}

/// eval_link/excl の値域上界（slot_count 個選んだ時に理論上到達しうる最大値、i64で計算）。
/// 上位 slot_count 件の値を合計した健全な上界（実際の到達可能値以上であることが保証される）。
fn value_bounds(prepared: &Prepared, slot_count: usize) -> (i64, i64) {
    let mut eval_vals: Vec<i32> = Vec::with_capacity(prepared.cands.len());
    let mut excl_vals: Vec<i32> = Vec::with_capacity(prepared.cands.len());
    for c in &prepared.cands {
        let mut e = 0i32;
        let mut x = 0i32;
        for &(idx, v) in &c.parts {
            if prepared.soft_excl_mask[idx as usize] {
                x += v;
            } else {
                e += v;
            }
        }
        eval_vals.push(e);
        excl_vals.push(x);
    }
    eval_vals.sort_unstable_by(|a, b| b.cmp(a));
    excl_vals.sort_unstable_by(|a, b| b.cmp(a));
    let eval_bound: i64 = eval_vals.iter().take(slot_count).map(|&v| v as i64).sum();
    let excl_bound: i64 = excl_vals.iter().take(slot_count).map(|&v| v as i64).sum();
    (eval_bound, excl_bound)
}

fn mask_bits(mask: &[bool]) -> u32 {
    mask.iter()
        .enumerate()
        .fold(0u32, |acc, (i, &b)| if b { acc | (1u32 << i) } else { acc })
}

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn push_i32(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

/// C(i,j) テーブル（i=0..n, j=0..slot_count）を u32 でシリアライズする。
/// optimize.wgsl の unrank（combinadic 展開）が参照する値と同じテーブル。
fn build_binom_table(n: usize, slot_count: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(n * slot_count * 4);
    for i in 0..n {
        for j in 0..slot_count {
            let c = optimizer::n_choose_k(i, j);
            if c > u32::MAX as u64 {
                return Err(format!("binomテーブルの値がu32を超えます C({i},{j})={c}"));
            }
            push_u32(&mut bytes, c as u32);
        }
    }
    Ok(bytes)
}

/// 探索順（w(m)降順）に並んだ密表現 cands を、モジュールごと固定 [`MAX_PARTS`] 枠へ
/// シリアライズする（不足分は attr_idx=0xFFFFFFFF の番兵で埋める）。
fn build_cand_parts(cands: &[Cand]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(cands.len() * MAX_PARTS * 8);
    for c in cands {
        for p in 0..MAX_PARTS {
            match c.parts.get(p) {
                Some(&(idx, val)) => {
                    push_u32(&mut bytes, idx);
                    push_i32(&mut bytes, val);
                }
                None => {
                    push_u32(&mut bytes, 0xFFFF_FFFF);
                    push_i32(&mut bytes, 0);
                }
            }
        }
    }
    bytes
}

/// requirements（下限Lv要求）を (attr_idx, min_lv) の u32 ペア列へシリアライズする。
/// 空の場合もダミー1件を積む（req_count=0 のため参照されないが、0件の storage buffer は
/// 一部バックエンドで無効になりうるため）。
fn build_req_entries(required_idxs: &[(usize, usize)]) -> Vec<u8> {
    if required_idxs.is_empty() {
        let mut bytes = Vec::with_capacity(8);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        return bytes;
    }
    let mut bytes = Vec::with_capacity(required_idxs.len() * 8);
    for &(idx, lv) in required_idxs {
        push_u32(&mut bytes, idx as u32);
        push_u32(&mut bytes, lv as u32);
    }
    bytes
}

/// requirements 属性ごとに、その属性の合計値が大きい順の上位 [`SEED_TOP_PER_REQUIRED_ATTR`]
/// 件と、w(m) 降順上位 [`SEED_TOP_BY_WEIGHT`] 件との和集合（sorted-order position の集合、
/// 昇順）を作る。CPUシードをこの集合に絞ることで、requirements があっても閾値算出に
/// 使える暫定 top-k が見つかりやすくなる（w(m) 降順上位だけでは、requirements 属性の値が
/// 低い候補ばかりで占められ、要求を満たす解が全く見つからない場合がある）。
fn build_seed_positions(prepared: &Prepared) -> Vec<usize> {
    let n = prepared.cands.len();
    let mut positions: std::collections::BTreeSet<usize> = (0..n.min(SEED_TOP_BY_WEIGHT)).collect();

    for &(attr_idx, _) in &prepared.required_idxs {
        let mut by_attr: Vec<(usize, i32)> = (0..n)
            .map(|pos| {
                let v = prepared.cands[pos]
                    .parts
                    .iter()
                    .find(|&&(idx, _)| idx as usize == attr_idx)
                    .map_or(0, |&(_, v)| v);
                (pos, v)
            })
            .collect();
        // 値降順・同点は position 昇順（決定的）。
        by_attr.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        positions.extend(
            by_attr
                .into_iter()
                .take(SEED_TOP_PER_REQUIRED_ATTR)
                .map(|(p, _)| p),
        );
    }

    positions.into_iter().collect()
}

/// フィールド順は optimize.wgsl の `Params` struct と完全一致させること（手動シリアライズ、
/// bytemuck 非依存）。
#[allow(clippy::too_many_arguments)]
fn build_params_bytes(
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
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(11 * 4);
    for v in [
        n,
        slot_count,
        n_attr,
        prefix_count,
        wg_x,
        table_cols,
        selected_mask,
        soft_excl_mask,
        req_count,
        threshold_hi,
        threshold_lo,
    ] {
        push_u32(&mut bytes, v);
    }
    bytes
}

/// GPU バッファから CPU メモリへ同期的に読み戻す（staging buffer + map_async + device.poll）。
fn read_buffer_bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    src: &wgpu::Buffer,
    offset: u64,
    size: u64,
) -> Result<Vec<u8>, String> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback-staging"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_buffer_to_buffer(src, offset, &staging, 0, size);
    queue.submit(Some(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| format!("device.poll失敗: {e}"))?;
    rx.recv()
        .map_err(|e| format!("readback受信失敗: {e}"))?
        .map_err(|e| format!("buffer map失敗: {e}"))?;
    // get_mapped_range() の一時的な BufferView は to_vec() でコピーした時点で不要になる
    // （この文末で drop される）。unmap() 前に明示的な drop は不要。
    let data = slice.get_mapped_range().to_vec();
    staging.unmap();
    Ok(data)
}

fn read_u32(device: &wgpu::Device, queue: &wgpu::Queue, buf: &wgpu::Buffer) -> Result<u32, String> {
    let bytes = read_buffer_bytes(device, queue, buf, 0, 4)?;
    let arr: [u8; 4] = bytes
        .try_into()
        .map_err(|_| "counter読込サイズ不正".to_string())?;
    Ok(u32::from_le_bytes(arr))
}

fn read_combos(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buf: &wgpu::Buffer,
    count: usize,
) -> Result<Vec<u32>, String> {
    let size = (count * 5 * 4) as u64;
    let bytes = read_buffer_bytes(device, queue, buf, 0, size)?;
    Ok(bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().expect("chunks_exact(4)は常に4バイト")))
        .collect())
}

/// GPU探索本体。失敗時は Err（呼び出し側の [`optimize`] が CPU フォールバックする）。
fn run_gpu_search(
    ctx: &GpuContext,
    prepared: &Prepared,
    top_k: usize,
    slot_count: usize,
) -> Result<Vec<Ranked>, String> {
    let run_start = std::time::Instant::now();
    let n = prepared.cands.len();
    let total_combinations = optimizer::n_choose_k(n, slot_count);

    // シード（CPU）: 「w(m)降順上位 min(n,60) 件」∪「requirements 属性ごとの上位10件」で
    // 厳密探索し、暫定 top-k の k位キーを閾値化する。シードの結果自体は topk へ投入しない
    // （GPU は常に「全 n 候補」を対象に全数評価するため、シードが見つけた combo は必ず
    // GPU 側でも再発見される — 部分集合は全候補の部分集合であり、シードの top-k は
    // 真の top-k 以下という単調性で保証される。ここで先に投入すると、GPU が同じ combo を
    // 再発見した際に重複エントリが生じる、部分集合が探索空間全体を覆うケースで顕在化する）。
    let seed_positions = build_seed_positions(prepared);
    let seed_prepared = prepared.subset(&seed_positions);
    let seed_ranked = optimizer::search_cpu(&seed_prepared, top_k, slot_count, true, true);

    let mut topk = TopK::new(top_k);

    let threshold: (u32, u32) = if seed_ranked.len() >= top_k {
        pack_key(&seed_ranked[top_k - 1].key)
    } else {
        // シード部分集合だけでは top_k 件すら見つからない稀なケース。安全側に倒し、
        // 閾値なし（=すべて拾う）から始める。
        (0, 0)
    };

    // アップロードするデータ（この関数呼び出し内では不変）。
    let binom_bytes = build_binom_table(n, slot_count)?;
    let cand_bytes = build_cand_parts(&prepared.cands);
    let req_bytes = build_req_entries(&prepared.required_idxs);
    let req_count = if prepared.required_idxs.is_empty() {
        0u32
    } else {
        prepared.required_idxs.len() as u32
    };
    let selected_mask_bits = mask_bits(&prepared.selected_mask);
    let soft_excl_mask_bits = mask_bits(&prepared.soft_excl_mask);

    let binom_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("binom"),
            contents: &binom_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
    let cand_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cand_parts"),
            contents: &cand_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
    let req_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("requirements"),
            contents: &req_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
    let out_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("out_combos"),
        size: u64::from(CAPACITY) * 5 * 4,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let counter_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("counter"),
        size: 4,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let table_cols = slot_count as u32;
    // プレフィックス数 P = C(n, slot_count-1)。MAX_N=300 の事前フォールバック判定により
    // u32 に確実に収まる（slot5・n=300 でも約3.3億）。
    let prefix_count_u64 = optimizer::n_choose_k(n, slot_count - 1);
    if prefix_count_u64 > u32::MAX as u64 {
        return Err(format!(
            "プレフィックス数がu32を超えます(n={n}, slot_count={slot_count}, P={prefix_count_u64})"
        ));
    }
    let prefix_count = prefix_count_u64 as u32;

    let wg_needed = prefix_count.div_ceil(256).max(1);
    let wg_x = wg_needed.min(65535);
    let wg_y = wg_needed.div_ceil(wg_x);
    if wg_y > 65535 {
        return Err(format!(
            "2Dディスパッチ上限を超えています(wg_x={wg_x}, wg_y={wg_y})"
        ));
    }

    // 単一ディスパッチ（チャンク分割なし。3節参照）。
    let dispatch_start = std::time::Instant::now();
    ctx.queue.write_buffer(&counter_buf, 0, &0u32.to_le_bytes());

    let params_bytes = build_params_bytes(
        n as u32,
        slot_count as u32,
        prepared.n_attr as u32,
        prefix_count,
        wg_x,
        table_cols,
        selected_mask_bits,
        soft_excl_mask_bits,
        req_count,
        threshold.0,
        threshold.1,
    );
    let params_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("params"),
            contents: &params_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("optimize-bg"),
        layout: &ctx.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: binom_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: cand_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: req_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: out_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: counter_buf.as_entire_binding(),
            },
        ],
    });

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("optimize-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&ctx.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(wg_x, wg_y, 1);
    }
    ctx.queue.submit(Some(encoder.finish()));

    let counter_value = read_u32(&ctx.device, &ctx.queue, &counter_buf)?;
    let gpu_elapsed = dispatch_start.elapsed();

    // オーバーフロー（counter > 容量）はそのままCPUフォールバックへ回す。atomicAdd の到達順は
    // 実行のたびに変わりうる任意サブセットであり、閾値以上の中の「上位」を保証しないため、
    // 部分出力から閾値を引き締めて再試行するのは不健全（真の解を取りこぼしうる）。
    if counter_value > CAPACITY {
        return Err(format!(
            "appendバッファがオーバーフローしました(counter={counter_value}, capacity={CAPACITY})"
        ));
    }
    let appended = counter_value;

    if appended > 0 {
        let combos = read_combos(&ctx.device, &ctx.queue, &out_buf, appended as usize)?;
        for raw in combos.chunks_exact(5) {
            let sorted_indices = &raw[..slot_count];
            if sorted_indices.iter().any(|&si| si as usize >= n) {
                return Err("GPU出力comboのインデックスが探索範囲外です（実行時異常）".to_string());
            }
            let mut acc = Accum::new(prepared.n_attr);
            for &si in sorted_indices {
                acc.add(
                    &prepared.cands[si as usize],
                    &prepared.selected_mask,
                    &prepared.soft_excl_mask,
                );
            }
            let key = acc.key();
            let mut combo: Vec<u32> = sorted_indices
                .iter()
                .map(|&si| prepared.order[si as usize] as u32)
                .collect();
            combo.sort_unstable();
            topk.offer(key, &combo);
        }
    }

    let mut ranked = topk.into_vec();
    ranked.sort_by(|a, b| b.cmp(a));
    ranked.truncate(top_k);

    log::info!(
        "[gpu] n={n} slot={slot_count} top_k={top_k} combos={total_combinations} \
         seed_solutions={} appended={appended} gpu={gpu_elapsed:?} total={:?}",
        seed_ranked.len(),
        run_start.elapsed()
    );

    Ok(ranked)
}

#[allow(clippy::too_many_arguments)]
fn cpu_fallback(
    modules: &[Module],
    selected_ids: &[i32],
    category: Option<&str>,
    hard_exclude_ids: &[i32],
    soft_exclude_ids: &[i32],
    requirements: &[(i32, usize)],
    top_k: usize,
    slot_count: usize,
    reason: &str,
) -> Result<OptimizeResult, String> {
    log::warn!("[gpu] {reason} のためCPU探索へフォールバックします");
    optimizer::optimize(
        modules,
        selected_ids,
        category,
        hard_exclude_ids,
        soft_exclude_ids,
        requirements,
        top_k,
        slot_count,
    )
}

/// GPU探索の公開API。シグネチャ・結果は [`crate::optimizer::optimize`] と同一。
/// どんな失敗（デバイス初期化失敗・値域超過・実行時エラー/panic）でもユーザーへエラーを
/// 返さず、CPU探索（[`crate::optimizer::optimize`]）へ委譲して完遂する。
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
) -> Result<OptimizeResult, String> {
    // prepare は CPU/GPU 共通ロジック。バリデーションエラーはフォールバックせずそのまま返す
    // （入力エラーはCPU版でも同じ結果になるため、GPU固有の問題ではない）。
    let prepared = optimizer::prepare(
        modules,
        selected_ids,
        category,
        hard_exclude_ids,
        soft_exclude_ids,
        requirements,
        slot_count,
    )?;

    if prepared.trivially_empty {
        return Ok(optimizer::assemble(&prepared, Vec::new()));
    }

    let n = prepared.cands.len();

    let fallback_reason = if prepared.n_attr > 32 {
        Some(format!(
            "属性数が32を超えています(n_attr={})",
            prepared.n_attr
        ))
    } else if top_k > 64 {
        Some(format!("top_kが64を超えています(top_k={top_k})"))
    } else if !(4..=5).contains(&slot_count) {
        Some(format!("slot_countが4/5以外です(slot_count={slot_count})"))
    } else if n > MAX_N {
        // optimize.wgsl はプレフィックス評価用の候補データをワークグループ共有メモリ
        // （固定長 MAX_N）へ協調ロードする。この上限を超える候補数は扱えない。
        Some(format!("候補数が多すぎます(n={n} > {MAX_N})"))
    } else if prepared.cands.iter().any(|c| c.parts.len() > MAX_PARTS) {
        Some(format!(
            "モジュールのパーツ数がGPUカーネルの前提({MAX_PARTS})を超えています"
        ))
    } else {
        let (eval_bound, excl_bound) = value_bounds(&prepared, slot_count);
        if eval_bound > 0xFFFF || excl_bound > 0xFFFF {
            Some(format!(
                "eval_link/excl の理論上界が0xFFFFを超えています(eval={eval_bound}, excl={excl_bound})"
            ))
        } else {
            None
        }
    };

    if let Some(reason) = fallback_reason {
        return cpu_fallback(
            modules,
            selected_ids,
            category,
            hard_exclude_ids,
            soft_exclude_ids,
            requirements,
            top_k,
            slot_count,
            &reason,
        );
    }

    let ctx = match gpu_context() {
        Ok(ctx) => ctx,
        Err(e) => {
            return cpu_fallback(
                modules,
                selected_ids,
                category,
                hard_exclude_ids,
                soft_exclude_ids,
                requirements,
                top_k,
                slot_count,
                &format!("GPU初期化に失敗しました: {e}"),
            );
        }
    };

    // GPU実行中のpanic（デバイスロスト等、wgpuはResultではなくpanicで報告することがある）も
    // 捕捉してCPUフォールバックへ回す。パニック後にGPUステート自体を再利用しないため
    // AssertUnwindSafe で安全。
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_gpu_search(ctx, &prepared, top_k, slot_count)
    }));

    let ranked = match result {
        Ok(Ok(ranked)) => ranked,
        Ok(Err(e)) => {
            return cpu_fallback(
                modules,
                selected_ids,
                category,
                hard_exclude_ids,
                soft_exclude_ids,
                requirements,
                top_k,
                slot_count,
                &format!("GPU実行エラー: {e}"),
            );
        }
        Err(_) => {
            return cpu_fallback(
                modules,
                selected_ids,
                category,
                hard_exclude_ids,
                soft_exclude_ids,
                requirements,
                top_k,
                slot_count,
                "GPU実行中にpanicが発生しました",
            );
        }
    };

    Ok(optimizer::assemble(&prepared, ranked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::{category_of, Part};
    use std::time::Instant;

    #[derive(serde::Deserialize)]
    struct DumpPart {
        attr_id: i32,
        #[serde(default)]
        attr_name: String,
        value: i32,
    }
    #[derive(serde::Deserialize)]
    struct DumpModule {
        key: i64,
        #[serde(default)]
        uuid: i64,
        config_id: i32,
        #[serde(default)]
        name: String,
        #[serde(default)]
        quality: i32,
        parts: Vec<DumpPart>,
    }

    fn load_dump(path: &str) -> Vec<Module> {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("読込失敗 {path}: {e}"));
        let raw: Vec<DumpModule> = serde_json::from_str(&text).expect("JSON 解析失敗");
        raw.into_iter()
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
            .collect()
    }

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

    /// GPU版・CPU版の solutions 列（モジュールkey列＋全メトリクス）が完全一致することを検証する。
    fn assert_gpu_matches_cpu(
        modules: &[Module],
        selected_ids: &[i32],
        soft_exclude_ids: &[i32],
        requirements: &[(i32, usize)],
        top_k: usize,
        slot_count: usize,
        ctx_label: &str,
    ) {
        let cpu = optimizer::optimize(
            modules,
            selected_ids,
            Some("all"),
            &[],
            soft_exclude_ids,
            requirements,
            top_k,
            slot_count,
        )
        .unwrap_or_else(|e| panic!("CPU optimize failed [{ctx_label}]: {e}"));

        let t = Instant::now();
        let gpu = optimize(
            modules,
            selected_ids,
            Some("all"),
            &[],
            soft_exclude_ids,
            requirements,
            top_k,
            slot_count,
        )
        .unwrap_or_else(|e| panic!("GPU optimize failed [{ctx_label}]: {e}"));
        let dt = t.elapsed();
        eprintln!(
            "[gpu-eq] {ctx_label}: modules={} slot={slot_count} top_k={top_k} solutions={} elapsed={dt:?}",
            modules.len(),
            gpu.solutions.len()
        );

        assert_eq!(
            cpu.candidate_count, gpu.candidate_count,
            "candidate_count mismatch [{ctx_label}]"
        );
        assert_eq!(
            cpu.combinations, gpu.combinations,
            "combinations mismatch [{ctx_label}]"
        );
        assert_eq!(
            cpu.solutions.len(),
            gpu.solutions.len(),
            "solutions件数 mismatch [{ctx_label}]"
        );
        for (i, (c, g)) in cpu.solutions.iter().zip(gpu.solutions.iter()).enumerate() {
            let ctx = format!("{ctx_label} rank={i}");
            let c_keys: Vec<i64> = c.modules.iter().map(|m| m.key).collect();
            let g_keys: Vec<i64> = g.modules.iter().map(|m| m.key).collect();
            assert_eq!(c_keys, g_keys, "module key列 mismatch [{ctx}]");
            assert_eq!(c.link_effect, g.link_effect, "link_effect mismatch [{ctx}]");
            assert_eq!(c.eval_link, g.eval_link, "eval_link mismatch [{ctx}]");
            assert_eq!(c.lv6_count, g.lv6_count, "lv6_count mismatch [{ctx}]");
            assert_eq!(c.lv5_count, g.lv5_count, "lv5_count mismatch [{ctx}]");
            assert_eq!(
                c.selected_lv6, g.selected_lv6,
                "selected_lv6 mismatch [{ctx}]"
            );
            assert_eq!(
                c.selected_present, g.selected_present,
                "selected_present mismatch [{ctx}]"
            );
            assert_eq!(c.level_sum, g.level_sum, "level_sum mismatch [{ctx}]");
        }
    }

    /// 実データ・合成データ（142/200/230件）× slot4/5 × top_k{3,10,100} ×
    /// {目標属性あり/なし} × {soft除外あり} × {requirementsあり} の代表組合せで
    /// GPU版とCPU版が完全一致することを検証する。実GPU必須のため #[ignore]。
    #[test]
    #[ignore]
    fn gpu_matches_cpu_real_and_synthetic() {
        let _ = env_logger::builder().is_test(true).try_init();

        let dump_path = std::env::var("BPSR_MODULE_DUMP")
            .unwrap_or_else(|_| "../../extracted_game_data/owned_modules.json".to_string());
        let dump_200 = std::env::var("BPSR_MODULE_DUMP_200").ok();
        let dump_230 = std::env::var("BPSR_MODULE_DUMP_230").ok();

        let mut datasets: Vec<(&str, Vec<Module>)> = vec![("real142", load_dump(&dump_path))];
        if let Some(p) = &dump_200 {
            datasets.push(("synth200", load_dump(p)));
        }
        if let Some(p) = &dump_230 {
            datasets.push(("synth230", load_dump(p)));
        }

        // 目標属性は 2104(極・ダメージ増強)。ソフト除外は 1113(特攻ダメージ強化)。
        // requirements は 1110(筋力強化) に Lv1 以上（大半のデータで満たせる緩い制約）。
        for (label, modules) in &datasets {
            for &slot in &[4usize, 5usize] {
                for &top_k in &[3usize, 10, 100] {
                    let ctx = format!("{label} slot{slot} k{top_k} plain");
                    assert_gpu_matches_cpu(modules, &[], &[], &[], top_k, slot, &ctx);

                    let ctx = format!("{label} slot{slot} k{top_k} selected");
                    assert_gpu_matches_cpu(modules, &[2104], &[], &[], top_k, slot, &ctx);

                    let ctx = format!("{label} slot{slot} k{top_k} soft_exclude");
                    assert_gpu_matches_cpu(modules, &[2104], &[1113], &[], top_k, slot, &ctx);

                    let ctx = format!("{label} slot{slot} k{top_k} requirements");
                    assert_gpu_matches_cpu(modules, &[2104], &[], &[(1110, 1)], top_k, slot, &ctx);
                }
            }
        }
    }

    /// 少数モジュールの手作りエッジケース: 同点キーのタイブレーク（combo 昇順優先）が
    /// GPU経由でも保たれることを検証する。
    #[test]
    #[ignore]
    fn gpu_matches_cpu_tie_break_edge_case() {
        let _ = env_logger::builder().is_test(true).try_init();
        // 4モジュールとも attr=101 の値20（同一Lv6・同一キー）。4枠なら全部採用で1解のみ。
        // 5モジュール目を追加し slot=4 にすると、どの4つを選んでもキーは同点になり、
        // combo（モジュールkey昇順）で決定的にタイブレークされることを検証する。
        const A: i32 = 101;
        let modules = vec![
            module(1, 5500103, &[(A, 20)]),
            module(2, 5500103, &[(A, 20)]),
            module(3, 5500103, &[(A, 20)]),
            module(4, 5500103, &[(A, 20)]),
            module(5, 5500103, &[(A, 20)]),
        ];
        assert_gpu_matches_cpu(&modules, &[], &[], &[], 10, 4, "tie_break slot4");
        assert_gpu_matches_cpu(&modules, &[], &[], &[], 10, 5, "tie_break slot5");
    }
}
