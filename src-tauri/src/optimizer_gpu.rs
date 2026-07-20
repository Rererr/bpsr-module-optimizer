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
//!    6カウンタを増分更新する（「1組合せ=1スレッド」方式で全スレッドが O(n) の unrank を
//!    行っていたコストを、プレフィックスあたり1回に削減）。候補データはワークグループ共有
//!    メモリへ協調ロードする。総プレフィックス数 P=C(n,slot_count-1) は n<=300 なら u32 に
//!    確実に収まるため、チャンク分割は不要（1ディスパッチで完結する）。
//!    プレフィックス確定後・末尾ループ開始前には、以下の**プレフィックス単位枝刈り**
//!    （[`build_suffix_max_bytes`]、`Params.prune_enabled`でON/OFF切替可）も行う:
//!    [`crate::optimizer::should_prune`] の r=1（残り1枠）相当の上界キーを suffix 最大値
//!    テーブルから計算し、CPUシード閾値を厳密に下回るなら（等号は絶対に刈らない）末尾
//!    ループを丸ごとスキップする。各成分は「suffix 最大値1個を独立に楽観視」した健全な
//!    上界（過小評価は絶対にしない）であり、CPU の should_prune と同じロジック・同じ
//!    半開区間セマンティクスを踏襲する。
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
//! - 属性値に負値が含まれる（キー packing は非負値を前提とするため。ゲーム側は常に非負の
//!   はずだが、将来データが崩れた場合の安全弁として追加）
//! - eval_link の理論上界が [`RankMode`] ごとの上界（`Link`=0x3FFF、`Lv5`=0xFFFF）を、
//!   excl の理論上界が 0xFFFF を超える（[`pack_key`] のキー packing の bit 幅を超える）
//! - GPU 実行時エラー・panic（デバイスロスト、シェーダ実行時エラー等）

use crate::optimizer::{
    self, Accum, Cand, Key, Module, OptimizeResult, Prepared, RankMode, Ranked, TopK,
};
use std::collections::HashSet;
use std::sync::OnceLock;
use wgpu::util::DeviceExt;

/// GPUカーネルが前提とするモジュール1件あたりの最大パーツ数（optimize.wgsl の
/// `MAX_PARTS` と一致させること）。実データ・合成データとも現状は最大3だが、
/// 将来のゲームデータ変更で超過した場合は静かに切り捨てず CPU へフォールバックする。
const MAX_PARTS: usize = 3;

/// append バッファの容量（optimize.wgsl の `CAPACITY` と一致させること）。
const CAPACITY: u32 = 2_097_152;

/// キー packing（[`pack_key`]）で eval_link に割り当てる bit 幅は [`RankMode`] により異なる
/// （[`pack_key`] のビット配置図参照）。理論上界がこれを超えるクエリは呼び出し側
/// （`value_bounds` 判定、[`eval_link_key_max`]）で CPU へフォールバックするため、この幅は
/// 「実データで十分な余裕を持つ固定値」であって理論上の絶対上限の証明ではない。
/// `Link`: hi 側の空き14bitに詰める（sel_present/sel_lv6/lv6 が hi の残りを占有するため）。
const EVAL_LINK_KEY_BITS_LINK: u32 = 14;
/// eval_link が `Link` モードのキー packing に収まる理論上界（2^14 - 1 = 16383）。
const EVAL_LINK_KEY_MAX_LINK: i64 = (1i64 << EVAL_LINK_KEY_BITS_LINK) - 1;
/// eval_link が `Lv5` モードのキー packing に収まる理論上界。`Lv5` モードでは eval_link を
/// lo の16bit全体に置く（level_sum 概念削除前の旧レイアウトを踏襲。実運用で長期間
/// 検証済みの配置であり、`Link` モードの14bitより大幅に余裕がある）。
const EVAL_LINK_KEY_MAX_LV5: i64 = 0xFFFF;
/// excl（ソフト除外合計）がキー packing に収まる理論上界（lo の下位16bit、両モード共通）。
const EXCL_KEY_MAX: i64 = 0xFFFF;

/// モードごとの eval_link 上界（[`value_bounds`] のフォールバック判定に使う）。
fn eval_link_key_max(mode: RankMode) -> i64 {
    match mode {
        RankMode::Link => EVAL_LINK_KEY_MAX_LINK,
        RankMode::Lv5 => EVAL_LINK_KEY_MAX_LV5,
    }
}

/// GPUカーネルが候補データをワークグループ共有メモリへ協調ロードする際の固定長上限
/// （optimize.wgsl の `MAX_N` と一致させること）。n がこれを超えたらCPUへフォールバックする。
/// n<=300 なら slot5 のプレフィックス数 C(300,4)≈3.3億 も u32 に確実に収まる。
const MAX_N: usize = 300;

/// CPUシードに requirements 属性ごとの上位を追加する件数（`build_seed_positions` 参照）。
const SEED_TOP_PER_REQUIRED_ATTR: usize = 10;

/// CPUシードの w(m) 降順上位件数（`build_seed_positions` 参照）。
const SEED_TOP_BY_WEIGHT: usize = 60;

/// 2フェーズカーネル（Phase B2、[`run_gpu_search_chunked`]）のチャンクサイズ（プレフィックス数）。
/// 2^23=8,388,608。workgroup_size=256 換算で 32768 workgroups となり、1D dispatch 上限
/// （65535）に対して、チャンク内が全件生存する最悪ケース（Kernel P・Kernel T どちらの
/// dispatch も）でも収まる安全マージンを持つ（65535*256=16,776,960 が理論上限）。
/// survivors バッファはこのサイズ分（8M*4B=32MiB）を確保して全チャンクで使い回す。
const CHUNK_SIZE: u32 = 8_388_608;

/// 中間閾値リファインメント（[`run_gpu_search_chunked`]）で、閾値を締め直すために
/// 「まとめて submit してから1回だけ poll する」先頭チャンクの個数。プレフィックスrankは
/// 探索順（w(m)降順）に対応するため良い解は先頭チャンクに集中するが、チャンク0（1個）だけ
/// では全体に占める割合が小さすぎ（n=300 slot5 で 8M/331M≈2.4%）、閾値の締まりが弱すぎて
/// requirements 条件の枝刈り率をほぼ改善できないことが実測で判明した。複数チャンクを
/// 1D dispatch 上限（65535 workgroups、CHUNK_SIZE 単位なら安全）内に収まる形で連続 submit
/// してから1回だけ poll すれば、同期回数を増やさずカバー率を上げられる。
const REFINE_CHUNKS: u32 = 4;

struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// 単一パスカーネル（Phase B、optimize.wgsl）。デバッグ/比較経路として温存する
    /// （[`GpuVariant::SinglePassPruned`]/[`GpuVariant::SinglePassUnpruned`]）。
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    /// 2フェーズカーネル（Phase B2、optimize_chunked.wgsl）。本番はこちらを使う
    /// （[`GpuVariant::Chunked`]）。P/I/T は同一シェーダモジュールだが、indirect dispatch
    /// args バッファ（`indirect_args`）を Kernel T の bind group に含めると、同一
    /// dispatch コマンド内で「shader read_write」と「indirect args 読み出し」の usage が
    /// 競合し wgpu のバリデーションで拒否されるため、P/I/T はそれぞれ専用の
    /// bind_group_layout・pipeline_layout を持つ（各々が実際に参照するバインディングのみを
    /// 含む部分集合。binding 番号自体は optimize_chunked.wgsl の宣言と一致させる）。
    p_bind_group_layout: wgpu::BindGroupLayout,
    i_bind_group_layout: wgpu::BindGroupLayout,
    t_bind_group_layout: wgpu::BindGroupLayout,
    p_pipeline: wgpu::ComputePipeline,
    i_pipeline: wgpu::ComputePipeline,
    t_pipeline: wgpu::ComputePipeline,
}

static GPU_CONTEXT: OnceLock<Result<GpuContext, String>> = OnceLock::new();

/// GPU探索の実装バリアント。取りこぼし防止テストで複数の独立実装を比較するために分離する
/// （[`optimize_with_opts`] 参照）。`SinglePassPruned`/`SinglePassUnpruned` はテスト専用
/// （本番の [`optimize`] は常に `Chunked`）なので、非テストビルドでは未構築になる。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GpuVariant {
    /// 本番: 2フェーズ（Kernel P: プレフィルタ→stream compaction→Kernel T: 本体、
    /// チャンク処理。[`run_gpu_search_chunked`]）。
    Chunked,
    /// デバッグ/比較用: 単一パスカーネル、プレフィックス単位枝刈り有効（Phase B、
    /// [`run_gpu_search`]）。
    #[cfg_attr(not(test), allow(dead_code))]
    SinglePassPruned,
    /// デバッグ/比較用: 単一パスカーネル、枝刈り無効（Phase B以前の「素の」実装。
    /// 2フェーズとの等価性検証で最も単純な独立実装として基準に使う）。
    #[cfg_attr(not(test), allow(dead_code))]
    SinglePassUnpruned,
}

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
/// 呼び出し側がウィンドウタイトル更新の要否判定に使う）。
/// 成功時 true、失敗時 false を返す。
pub fn prewarm() -> bool {
    match gpu_context() {
        Ok(_) => {
            log::info!("[gpu] プリウォーム完了（デバイス初期化・パイプライン構築済み）");
            true
        }
        Err(e) => {
            log::warn!("[gpu] プリウォーム失敗（クエリ時にCPUへフォールバックします）: {e}");
            false
        }
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
            storage_entry(6, true),
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

    // 2フェーズカーネル（Phase B2、本番）。P/I/T は同一シェーダモジュールだが、
    // indirect_args バッファ（binding 8）を Kernel T の bind group に含めると usage 競合で
    // 拒否されるため（GpuContext のフィールドコメント参照）、各カーネルは実際に参照する
    // バインディングのみを含む専用レイアウトを持つ（binding 番号は wgsl 側の宣言と一致
    // させる。連番である必要はない）。
    let chunked_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("optimize-chunked-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("optimize_chunked.wgsl").into()),
    });

    // Kernel P: binom(0), cand_parts_g(1), params(2), requirements(3), suffix_max(4),
    // survivors(5), counters(6)。requirements(3) は requirements 実現可能性チェック用。
    let p_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("optimize-chunked-p-bgl"),
        entries: &[
            storage_entry(0, true),
            storage_entry(1, true),
            storage_entry(2, true),
            storage_entry(3, true),
            storage_entry(4, true),
            storage_entry(5, false),
            storage_entry(6, false),
        ],
    });
    // Kernel I: counters(6), indirect_args(8)。
    let i_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("optimize-chunked-i-bgl"),
        entries: &[storage_entry(6, false), storage_entry(8, false)],
    });
    // Kernel T: binom(0), cand_parts_g(1), params(2), requirements(3), survivors(5),
    // counters(6), out_combos(7)。indirect_args(8) は含めない。
    let t_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("optimize-chunked-t-bgl"),
        entries: &[
            storage_entry(0, true),
            storage_entry(1, true),
            storage_entry(2, true),
            storage_entry(3, true),
            storage_entry(5, false),
            storage_entry(6, false),
            storage_entry(7, false),
        ],
    });

    let p_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("optimize-chunked-p-pl"),
        bind_group_layouts: &[Some(&p_bind_group_layout)],
        immediate_size: 0,
    });
    let i_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("optimize-chunked-i-pl"),
        bind_group_layouts: &[Some(&i_bind_group_layout)],
        immediate_size: 0,
    });
    let t_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("optimize-chunked-t-pl"),
        bind_group_layouts: &[Some(&t_bind_group_layout)],
        immediate_size: 0,
    });

    let p_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("optimize-chunked-p-pipeline"),
        layout: Some(&p_pipeline_layout),
        module: &chunked_shader,
        entry_point: Some("main_p"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    let i_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("optimize-chunked-i-pipeline"),
        layout: Some(&i_pipeline_layout),
        module: &chunked_shader,
        entry_point: Some("main_i"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    let t_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("optimize-chunked-t-pipeline"),
        layout: Some(&t_pipeline_layout),
        module: &chunked_shader,
        entry_point: Some("main_t"),
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
        p_bind_group_layout,
        i_bind_group_layout,
        t_bind_group_layout,
        p_pipeline,
        i_pipeline,
        t_pipeline,
    })
}

/// キー（[`Key`]）を optimize.wgsl の hi/lo packing と同じビット割当で u32 2つへ詰める。
/// モードにより配置が異なる（optimize.wgsl / optimize_chunked.wgsl の `pack_hi_lo` と
/// 完全に一致させること。両ファイルとも「明示的な2分岐」で実装し、汎用シフト/マスクの
/// uniform 渡しは行わない — 静かに順序が壊れると「刈りすぎ＝最適解を無言で失う」箇所の
/// ため、分岐コストより可読性・レビュー容易性を優先する）:
///
/// - `Link`: hi = sel_present(6)|sel_lv6(6)|lv6(6)|eval_link(14)、
///   lo = lv5(16、実際に使う値は数個程度)|(0xFFFF-excl)(16)
/// - `Lv5` : hi = sel_present(6)|sel_lv6(6)|lv6(6)|lv5(6)|未使用(8)、
///   lo = eval_link(16)|(0xFFFF-excl)(16)
///
/// hi/lo の2語比較（hi優先→lo）がそのままキーの辞書式順序を保つのは、各成分がより上位の
/// 成分より必ず低いbit位置に詰まっているため。`Lv5` モードは level_sum 概念削除前の
/// 旧レイアウト（eval_link を lo の16bit全体に置く、実運用で長期間検証済みの配置）を
/// 踏襲する。
///
/// 各成分が対応 bit 幅に収まることは呼び出し側のフォールバック判定
/// （n_attr<=32・eval_link上界<=[`eval_link_key_max`]`(mode)`・excl上界<=[`EXCL_KEY_MAX`]）で
/// 事前に保証されている。lv5 は候補データの構造的上限（MAX_PARTS×slot_count<=15）から
/// 16bit（`Link`）/6bit（`Lv5`）どちらにも収まることが自明なため実行時チェックしない。
///
/// `assert` はその保証が破れていないかの唯一の保険。GPU探索の健全性が `value_bounds` に
/// よるフォールバックゲート1点に依存しており、ここが破れた場合の症状は panic やエラーでは
/// なく「eval_link の桁溢れが上位ビット（sel_present/lv6 等）を汚染し、最適解が誤って
/// 刈られて静かに消える」という気付きにくい形で出るため、将来ゲートを迂回する経路が
/// 増えても確実に検出できるようにしておく。GPU探索のテストは n=300 の実行時間の都合上
/// `--release` 前提で回すため、あえて `debug_assert!` ではなく `assert!` を使う
/// （`--release` では `debug_assert!` は evaluate すらされず、唯一のゲートの保険が
/// 事実上機能しないため）。呼び出しは1ディスパッチあたり O(1) でコストは無視できる。
fn pack_key(key: &Key, mode: RankMode) -> (u32, u32) {
    let sel_present = key[0] as u32;
    let sel_lv6 = key[1] as u32;
    let lv6 = key[2] as u32;
    // Accum::key/naive_ranked は -excl を格納しているので符号を戻す。
    let excl = (-key[5]) as u32;
    assert!(
        i64::from(excl) <= EXCL_KEY_MAX,
        "excl がキー packing の bit 幅を超えています (excl={excl}, EXCL_KEY_MAX={EXCL_KEY_MAX})"
    );
    match mode {
        RankMode::Link => {
            let eval_link = key[3] as u32;
            let lv5 = key[4] as u32;
            assert!(
                i64::from(eval_link) <= EVAL_LINK_KEY_MAX_LINK,
                "eval_link がキー packing の bit 幅を超えています \
                 (eval_link={eval_link}, EVAL_LINK_KEY_MAX_LINK={EVAL_LINK_KEY_MAX_LINK})"
            );
            let hi = (sel_present << 26) | (sel_lv6 << 20) | (lv6 << 14) | eval_link;
            let lo = (lv5 << 16) | (0xFFFFu32 - excl);
            (hi, lo)
        }
        RankMode::Lv5 => {
            let lv5 = key[3] as u32;
            let eval_link = key[4] as u32;
            assert!(
                i64::from(eval_link) <= EVAL_LINK_KEY_MAX_LV5,
                "eval_link がキー packing の bit 幅を超えています \
                 (eval_link={eval_link}, EVAL_LINK_KEY_MAX_LV5={EVAL_LINK_KEY_MAX_LV5})"
            );
            let hi = (sel_present << 26) | (sel_lv6 << 20) | (lv6 << 14) | (lv5 << 8);
            let lo = (eval_link << 16) | (0xFFFFu32 - excl);
            (hi, lo)
        }
    }
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

/// GPU側 Params.rank_mode のエンコード（optimize.wgsl / optimize_chunked.wgsl の
/// `RANK_MODE_LINK`/`RANK_MODE_LV5` 定数と一致させること）。
fn rank_mode_u32(mode: RankMode) -> u32 {
    match mode {
        RankMode::Link => 0,
        RankMode::Lv5 => 1,
    }
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

/// プレフィックス単位枝刈り（Phase B）用の suffix 最大値（r=1）テーブルを構築し、
/// GPU へアップロードするバイト列を返す。optimizer::AttrBounds/SuffixSum は DFS の
/// 任意深さ r で使う汎用 top-r テーブルだが、GPU カーネルはプレフィックス確定後に
/// 必ず残り1枠（末尾ループが選ぶ1候補）しか無いため、r=1（= suffix の単一最大値）
/// だけで足りる。DFS 用の複雑なソート挿入によるtop-r構築は不要で、末尾から前方への
/// 単純な running-max スキャンで計算できる。
///
/// レイアウト（1本の storage buffer にまとめてバインディング数を節約する）:
///   \[attr_suffix_max: n_attr*(n+1) i32\] ++ \[w_suffix_max: (n+1) i32\]
/// - `attr_suffix_max[a*(n+1)+s]` = `cands[s..n]` における属性 a の単一候補寄与値（同一候補の
///   複数パーツ合計、[`optimizer::AttrBounds`] の values と同じ定義）の最大値。ソフト除外属性の
///   行も一様に計算するが、カーネル側のプルーニングロジックは soft_excl_mask で弾いて参照しない。
/// - `w_suffix_max[s]` = counted w(m)（非ソフト除外パーツ値合計）の suffix 単一最大値
///   （評価リンク上界に使う）。
///
/// 位置 s=n（空 suffix）は常に0。
fn build_suffix_max_bytes(prepared: &Prepared) -> Vec<u8> {
    let n = prepared.cands.len();
    let n_attr = prepared.n_attr;
    let soft_excl_mask = &prepared.soft_excl_mask;

    // 候補ごとの寄与値（属性ごとの合計・counted w(m)）。
    let mut attr_val = vec![0i32; n * n_attr];
    let mut w_val = vec![0i32; n];
    for (i, c) in prepared.cands.iter().enumerate() {
        for &(a, v) in &c.parts {
            let a = a as usize;
            attr_val[i * n_attr + a] += v;
            if !soft_excl_mask[a] {
                w_val[i] += v;
            }
        }
    }

    // suffix 単一最大値（末尾 s=n から前方へ running max）。
    let mut attr_suffix_max = vec![0i32; n_attr * (n + 1)];
    let mut w_suffix_max = vec![0i32; n + 1];
    for s in (0..n).rev() {
        for a in 0..n_attr {
            let prev = attr_suffix_max[a * (n + 1) + s + 1];
            let here = attr_val[s * n_attr + a];
            attr_suffix_max[a * (n + 1) + s] = prev.max(here);
        }
        w_suffix_max[s] = w_suffix_max[s + 1].max(w_val[s]);
    }

    let mut bytes = Vec::with_capacity((attr_suffix_max.len() + w_suffix_max.len()) * 4);
    for &v in &attr_suffix_max {
        push_i32(&mut bytes, v);
    }
    for &v in &w_suffix_max {
        push_i32(&mut bytes, v);
    }
    bytes
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
    prune_enabled: u32,
    rank_mode: u32,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(13 * 4);
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
        prune_enabled,
        rank_mode,
    ] {
        push_u32(&mut bytes, v);
    }
    bytes
}

/// フィールド順は optimize_chunked.wgsl の `Params` struct と完全一致させること
/// （手動シリアライズ、bytemuck 非依存）。`chunk_start`/`chunk_count` はチャンク毎に
/// 呼び出し元が書き換えて使い回す想定（[`run_gpu_search_chunked`] 参照）。
#[allow(clippy::too_many_arguments)]
fn build_params_chunked_bytes(
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
    rank_mode: u32,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(12 * 4);
    for v in [
        n,
        slot_count,
        n_attr,
        table_cols,
        selected_mask,
        soft_excl_mask,
        req_count,
        threshold_hi,
        threshold_lo,
        chunk_start,
        chunk_count,
        rank_mode,
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

/// `offset` バイト目から u32 を1つ読む。counters バッファのレイアウトは呼び出し元の
/// カーネル構成によって異なる：
/// - 単一パス（`optimize.wgsl`、`array<atomic<u32>, 2>`）:
///   `[0]=appended件数, [4]=pruned プレフィックス件数`
/// - チャンク分割（`optimize_chunked.wgsl`、`array<atomic<u32>, 3>`）:
///   `[0]=appended件数(全チャンク累積), [4]=survivor_total(全チャンク累積・診断用),
///   [8]=chunk_survivor_count(チャンク毎にリセット)`
///
/// どちらの構成でも `[0]` は appended 件数を指すため offset で読み分ける。
fn read_u32_at(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buf: &wgpu::Buffer,
    offset: u64,
) -> Result<u32, String> {
    let bytes = read_buffer_bytes(device, queue, buf, offset, 4)?;
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

/// out_combos の生データ（1件=5要素のu32配列、先頭slot_count個が探索順indexの有効値、
/// 残りはNO_PARTパディング）を CPU 側 [`Accum`] で厳密再計算する（GPU 側の結果は一切
/// 信用しない）。combo は元index昇順にソート済みで返す。チャンク0の閾値リファインメント
/// 計算と、全チャンク投入後の最終マージの両方で共有するヘルパー。
fn recompute_combo(
    prepared: &Prepared,
    slot_count: usize,
    raw: &[u32],
    mode: RankMode,
) -> Result<(Key, Vec<u32>), String> {
    let n = prepared.cands.len();
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
    let key = acc.key(mode);
    let mut combo: Vec<u32> = sorted_indices
        .iter()
        .map(|&si| prepared.order[si as usize] as u32)
        .collect();
    combo.sort_unstable();
    Ok((key, combo))
}

/// 閾値リファインメント（[`run_gpu_search_chunked`]）用: シード top-k と追加combo群を
/// **重複を除いて**一時TopKへマージし、k位キー（cap件未満なら None）を返す。
///
/// 重複除去が必須の理由（admissible性の前提）: シードが見つけた combo は、シードが使う
/// 候補が低い探索順indexに位置しやすいため、先頭チャンク群のGPU全数評価でも高確率で
/// 「再発見」される（同一combo）。[`TopK::offer`] は combo の重複を検知せず、単に
/// 「現在の最劣キーより良いか」だけで判定するため、同一comboを重複してofferすると、
/// 本来保持すべき別の（真に異なる）低ランクcomboを誤って追い出してしまい、結果として
/// worst_key が「distinct top-kのk位」ではなく「重複を含むマルチセットのk位」になる
/// （真の全体k位を超えて締まりすぎうる＝admissible違反）。ここでは `extra` のうち
/// `seed_ranked` に既出の combo をスキップすることで、実質的に
/// distinct(`seed_ranked` ∪ `extra`) の top-k を計算する。
///
/// 前提（呼び出し元が保証すること）: `extra` 自体には重複が無いこと（GPU の1チャンク分の
/// append は、プレフィックスrankごとに Kernel P が高々1回・(prefix,last)の組ごとに
/// Kernel T が高々1回しか処理しないため、1チャンク内で同一comboが複数回appendされることは
/// 構造的に無い。複数チャンクをまとめて渡す場合はチャンク間のプレフィックス範囲が排他的
/// なため、そちらも重複しない）。`seed_ranked` 自体も探索方式上 combo の重複は無い
/// （厳密DFSは各comboを高々1回訪問する）。
fn merge_topk_for_refinement(
    seed_ranked: &[Ranked],
    extra: impl IntoIterator<Item = (Key, Vec<u32>)>,
    top_k: usize,
) -> Option<Key> {
    let seed_combo_set: HashSet<&[u32]> =
        seed_ranked.iter().map(|r| r.combo.as_slice()).collect();
    let mut topk = TopK::new(top_k);
    for r in seed_ranked {
        topk.offer(r.key, &r.combo);
    }
    for (key, combo) in extra {
        if seed_combo_set.contains(combo.as_slice()) {
            continue;
        }
        topk.offer(key, &combo);
    }
    topk.worst_key()
}

/// 1チャンク分の Kernel P→I→T を1つの encoder+submit にまとめて投げる（submit するだけで
/// 完了は待たない。呼び出し元が必要に応じて `device.poll` する）。チャンク0の単独実行
/// （閾値リファインメント用）と、以降チャンクのバッチ実行の両方で共有するヘルパー。
#[allow(clippy::too_many_arguments)]
fn dispatch_one_chunk(
    ctx: &GpuContext,
    p_bind_group: &wgpu::BindGroup,
    i_bind_group: &wgpu::BindGroup,
    t_bind_group: &wgpu::BindGroup,
    indirect_args_buf: &wgpu::Buffer,
    counters_buf: &wgpu::Buffer,
    params_buf: &wgpu::Buffer,
    params_bytes: &[u8],
    chunk_count: u32,
) {
    // counters[2]（チャンクローカル生存数）をチャンク毎に0リセット。
    ctx.queue
        .write_buffer(counters_buf, 2 * 4, &0u32.to_le_bytes());
    ctx.queue.write_buffer(params_buf, 0, params_bytes);

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        // Kernel P: プレフィルタ。chunk_count 件のプレフィックスを1Dディスパッチで評価
        // （chunk_count<=CHUNK_SIZE なので ceil(chunk_count/256)<=32768、1D上限内）。
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("p-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&ctx.p_pipeline);
        pass.set_bind_group(0, p_bind_group, &[]);
        let wg_p = chunk_count.div_ceil(256);
        pass.dispatch_workgroups(wg_p, 1, 1);
    }
    {
        // Kernel I: indirect dispatch args builder。別パスに分けることで、P の
        // counters[2] 書き込みと I の読み出しの間、および I の indirect_args 書き込みと
        // T の dispatch_workgroups_indirect 読み出しの間に、コンピュートパス境界という
        // 明確な同期点を置く（WebGPU のパス境界は書き込みの可視性を保証する）。
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("i-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&ctx.i_pipeline);
        pass.set_bind_group(0, i_bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    {
        // Kernel T: 本体。ワークグループ数は Kernel I が書いた indirect_args_buf から
        // 決まる（counters バッファとは別の専用バッファ。usage 競合を避けるため）。
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("t-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&ctx.t_pipeline);
        pass.set_bind_group(0, t_bind_group, &[]);
        pass.dispatch_workgroups_indirect(indirect_args_buf, 0);
    }
    ctx.queue.submit(Some(encoder.finish()));
}

/// GPU探索本体（Phase B2: 2フェーズ・チャンク処理）。失敗時は Err（呼び出し側の
/// [`optimize`] が CPU フォールバックする）。
///
/// Phase B（[`run_gpu_search`]、単一パス+プレフィックス単位枝刈り）は、ワープ内に1本でも
/// 生存者（末尾ループを最後まで実行するスレッド）が残るとワープ全体の完了時間が縮まらない
/// SIMT特性の限界に直面した（n=300 slot5 実測: 枝刈り率93%でも高速化はほぼゼロ）。この
/// 関数は、生存者だけを stream compaction で密に集めてから本処理する2カーネル分離
/// （optimize_chunked.wgsl の Kernel P → Kernel I → Kernel T）でこの限界を回避する。
///
/// n<=300 全域を [`CHUNK_SIZE`] 単位のチャンクへ分割し、チャンク毎に P→I→T を実行する。
/// Kernel T の起動ワークグループ数は Kernel I が GPU 側で書いた indirect dispatch args
/// から決まるため、チャンク間で CPU への読み戻しは発生しない（全チャンク投入後に
/// 1回だけ appended/survivor_total を読み戻す）。各チャンクは個別に `queue.submit` するが、
/// submit 自体は非同期（GPU完了を待たない）なので、チャンク間の CPU-GPU 同期ストールは
/// 発生しない（`queue.write_buffer` は同一キュー上で submit と順序保証されるため、
/// チャンク毎の params/counter リセットが前チャンドの処理を汚染することもない）。
///
/// **中間閾値リファインメント**: プレフィックス rank は探索順（w(m)降順）に対応するため、
/// 良い解の多くは先頭チャンクに集中する。そこで先頭 [`REFINE_CHUNKS`] 個をまとめて
/// submit してから1回だけ poll して完了を待ち、その実結果（厳密再計算済み）を CPU
/// シードの top-k とマージして、k位キーが求まればそれを「締め直した閾値」として残りの
/// チャンクへ適用する（求まらなければ従来のシード由来閾値のまま）。チャンク0（1個）
/// だけでは全体に占める割合が小さすぎ（n=300 slot5 で 8M/331M≈2.4%）閾値の締まりが
/// 弱すぎることが実測で判明したため、複数チャンクをまとめる設計にしている。
///
/// 正当性（admissible・取りこぼしゼロ）: シードtop-k と先頭チャンク群の実結果は、いずれも
/// 「全候補空間の部分集合」から得た厳密な値である。部分集合の top-k の k位キーは、
/// 全候補空間の真の k位キー以下（単調性: 候補を追加しても k位キーは同じか良くなる一方）。
/// したがって (シード ∪ 先頭チャンク群実結果) の k位キー（リファインメント後の閾値）も、
/// 真の全体 k位キー以下であることが保証される。真の top-k combo のキーは真の全体k位キー
/// 以上なので、リファインメント後の閾値（それ以下）でも必ず accept される
/// （`hi>threshold_hi || (hi==threshold_hi && lo>=threshold_lo)` という既存の半開区間
/// セマンティクスを閾値の値だけ差し替えて適用するため、等号の扱いも従来と同一）。
/// リファインメントは「残りのチャンク（REFINE_CHUNKS..N-1）」にのみ適用し、先頭チャンク群
/// 自身は既にその場で（リファインメント前の閾値で）確定済みなので再実行しない。先頭
/// チャンク群の実結果はリファインメント計算専用の一時 TopK にのみ投入し、最終結果用の
/// `topk` へは投入しない（最終読み戻しが全チャンク分の out_combos を一括処理する際に
/// 自然に含まれるため、ここで投入すると二重計上になる）。
fn run_gpu_search_chunked(
    ctx: &GpuContext,
    prepared: &Prepared,
    top_k: usize,
    slot_count: usize,
    mode: RankMode,
) -> Result<Vec<Ranked>, String> {
    let run_start = std::time::Instant::now();
    let n = prepared.cands.len();
    let total_combinations = optimizer::n_choose_k(n, slot_count);

    // シード（CPU）: run_gpu_search と同じロジックで閾値を得る。
    let seed_positions = build_seed_positions(prepared);
    let seed_prepared = prepared.subset(&seed_positions);
    let seed_ranked = optimizer::search_cpu(&seed_prepared, top_k, slot_count, mode, true, true);

    let mut threshold: (u32, u32) = if seed_ranked.len() >= top_k {
        pack_key(&seed_ranked[top_k - 1].key, mode)
    } else {
        (0, 0)
    };

    let binom_bytes = build_binom_table(n, slot_count)?;
    let cand_bytes = build_cand_parts(&prepared.cands);
    let req_bytes = build_req_entries(&prepared.required_idxs);
    let suffix_max_bytes = build_suffix_max_bytes(prepared);
    let req_count = if prepared.required_idxs.is_empty() {
        0u32
    } else {
        prepared.required_idxs.len() as u32
    };
    let selected_mask_bits = mask_bits(&prepared.selected_mask);
    let soft_excl_mask_bits = mask_bits(&prepared.soft_excl_mask);

    let table_cols = slot_count as u32;
    let prefix_count_u64 = optimizer::n_choose_k(n, slot_count - 1);
    if prefix_count_u64 > u32::MAX as u64 {
        return Err(format!(
            "プレフィックス数がu32を超えます(n={n}, slot_count={slot_count}, P={prefix_count_u64})"
        ));
    }
    let prefix_count = prefix_count_u64 as u32;

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
    let suffix_max_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suffix_max"),
            contents: &suffix_max_bytes,
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
    // survivors[i] = i番目に生存したプレフィックスの global rank。チャンクサイズ分の
    // 容量を確保して全チャンクで使い回す（生存者数はチャンク内プレフィックス数を超えない
    // ためオーバーフローが構造的に起きない）。
    let survivors_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("survivors"),
        size: u64::from(CHUNK_SIZE) * 4,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    // counters[0]=appended(全チャンク累積), counters[1]=survivor_total(全チャンク累積・診断用),
    // counters[2]=chunk_survivor_count(チャンク毎に0リセット)。indirect dispatch args は
    // usage 競合を避けるため別バッファ（indirect_args_buf）に分離する（GpuContext の
    // フィールドコメント・optimize_chunked.wgsl 参照）。
    let counters_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("counters_chunked"),
        size: 3 * 4,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // indirect dispatch args(x,y,z)。Kernel I が書き、CPU側の dispatch_workgroups_indirect
    // が直接読む（Kernel T の bind group には含めない）。
    let indirect_args_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("indirect_args"),
        size: 3 * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
        mapped_at_creation: false,
    });
    // params は毎チャンク chunk_start/chunk_count を書き換えて使い回す。
    let params_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("params_chunked"),
        size: 12 * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let p_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("optimize-chunked-p-bg"),
        layout: &ctx.p_bind_group_layout,
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
                resource: suffix_max_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: survivors_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: counters_buf.as_entire_binding(),
            },
        ],
    });
    let i_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("optimize-chunked-i-bg"),
        layout: &ctx.i_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 6,
                resource: counters_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: indirect_args_buf.as_entire_binding(),
            },
        ],
    });
    let t_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("optimize-chunked-t-bg"),
        layout: &ctx.t_bind_group_layout,
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
                binding: 5,
                resource: survivors_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: counters_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: out_buf.as_entire_binding(),
            },
        ],
    });

    // counters[0..2)（appended・survivor_total）は全チャンク累積のため最初の1回だけ0初期化。
    ctx.queue.write_buffer(&counters_buf, 0, &[0u8; 8]);

    let dispatch_start = std::time::Instant::now();
    let num_chunks = prefix_count.div_ceil(CHUNK_SIZE).max(1);

    // --- 先頭 REFINE_CHUNKS 個: まとめて submit してから1回だけ poll し、閾値
    //     リファインメントを行う（詳細は関数docの「中間閾値リファインメント」節・
    //     正当性節を参照）。---
    let num_refine_chunks = REFINE_CHUNKS.min(num_chunks);
    for chunk_idx in 0..num_refine_chunks {
        let chunk_start = chunk_idx * CHUNK_SIZE;
        let chunk_count = (prefix_count - chunk_start).min(CHUNK_SIZE);
        if chunk_count == 0 {
            break;
        }
        let params_bytes = build_params_chunked_bytes(
            n as u32,
            slot_count as u32,
            prepared.n_attr as u32,
            table_cols,
            selected_mask_bits,
            soft_excl_mask_bits,
            req_count,
            threshold.0,
            threshold.1,
            chunk_start,
            chunk_count,
            rank_mode_u32(mode),
        );
        dispatch_one_chunk(
            ctx,
            &p_bind_group,
            &i_bind_group,
            &t_bind_group,
            &indirect_args_buf,
            &counters_buf,
            &params_buf,
            &params_bytes,
            chunk_count,
        );
    }
    {
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("device.poll失敗（閾値リファインメント同期）: {e}"))?;

        let appended0 = read_u32_at(&ctx.device, &ctx.queue, &counters_buf, 0)?;
        if appended0 > CAPACITY {
            return Err(format!(
                "appendバッファがオーバーフローしました(counter={appended0}, capacity={CAPACITY})"
            ));
        }
        // 健全性ガード（低確率だが既存の atomicAdd 実装は理論上ありうる）: counters[0] は
        // u32 のため、accept 数が u32::MAX 近くに達するとラップアラウンドし、上の
        // `appended0 > CAPACITY` 比較を偽陰性ですり抜けうる（ラップ後の値がたまたま
        // CAPACITY 以下に着地するケース）。accept 数は全組み合わせ数 `total_combinations`
        // を物理的に超えられないため、ここでの矛盾はラップアラウンドの動かぬ証拠になる
        // （逆に矛盾が無いことはラップアラウンドが起きていない証明にはならない点に注意。
        // 完全な検知ではなく、明らかな異常を捉えるための追加ガード）。
        if u64::from(appended0) > total_combinations {
            return Err(format!(
                "appendedカウンタが理論上限を超えています(appended0={appended0}, \
                 total_combinations={total_combinations})。u32カウンタの桁溢れの可能性があります"
            ));
        }
        if appended0 > 0 {
            let combos0 = read_combos(&ctx.device, &ctx.queue, &out_buf, appended0 as usize)?;
            // シードtop-k と 先頭チャンク群の厳密再計算済み実結果を、重複除去しつつ
            // 一時TopKへマージする（最終結果用の topk へは投入しない。二重計上を避けるため
            // — 全チャンク投入後の最終読み戻しが先頭チャンク分も含めて自然にカバーする）。
            // 重複除去の理由は [`merge_topk_for_refinement`] のdoc参照（admissible性の前提）。
            let mut extra = Vec::with_capacity(appended0 as usize);
            for raw in combos0.chunks_exact(5) {
                extra.push(recompute_combo(prepared, slot_count, raw, mode)?);
            }
            if let Some(worst_key) = merge_topk_for_refinement(&seed_ranked, extra, top_k) {
                let refined = pack_key(&worst_key, mode);
                // 「ログに出すか」だけの条件分岐であり、`threshold` への代入は常に無条件で
                // 行う（refined が old と同値でも、明示的に再代入して以降のロジックが
                // 常に最新のリファインメント結果を参照するようにする。ログを出す/出さないは
                // 表示上の最適化に過ぎず、正当性には無関係）。
                if refined != threshold {
                    log::info!(
                        "[gpu-chunked] n={n} slot={slot_count} top_k={top_k} 閾値リファインメント \
                         refine_chunks={num_refine_chunks} appended0={appended0} \
                         old={threshold:?} new={refined:?}"
                    );
                }
                threshold = refined;
            }
        }
    }

    // --- 残りチャンク: リファインメント後の閾値で、従来どおりまとめて submit する
    //     （追加の同期なし）。---
    for chunk_idx in num_refine_chunks..num_chunks {
        let chunk_start = chunk_idx * CHUNK_SIZE;
        let chunk_count = (prefix_count - chunk_start).min(CHUNK_SIZE);
        if chunk_count == 0 {
            break;
        }
        let params_bytes = build_params_chunked_bytes(
            n as u32,
            slot_count as u32,
            prepared.n_attr as u32,
            table_cols,
            selected_mask_bits,
            soft_excl_mask_bits,
            req_count,
            threshold.0,
            threshold.1,
            chunk_start,
            chunk_count,
            rank_mode_u32(mode),
        );
        dispatch_one_chunk(
            ctx,
            &p_bind_group,
            &i_bind_group,
            &t_bind_group,
            &indirect_args_buf,
            &counters_buf,
            &params_buf,
            &params_bytes,
            chunk_count,
        );
    }

    // 全チャンク投入後、一度だけ読み戻す（チャンク間のCPU-GPU同期は発生させない）。
    // チャンク0の実結果もここで（再度）読み戻して厳密再計算するが、上のリファインメント
    // 計算は結果を専用の一時TopKにのみ投入しており最終結果用topkには一切触れていないため
    // 二重計上は起きない。
    let appended = read_u32_at(&ctx.device, &ctx.queue, &counters_buf, 0)?;
    let survivor_total = read_u32_at(&ctx.device, &ctx.queue, &counters_buf, 4)?;
    let gpu_elapsed = dispatch_start.elapsed();

    // オーバーフロー（appended > 容量）はそのままCPUフォールバックへ回す（run_gpu_search と
    // 同じ理由: atomicAdd の到達順は実行のたびに変わりうる任意サブセットのため、部分出力
    // から閾値を引き締めて再試行するのは不健全）。
    if appended > CAPACITY {
        return Err(format!(
            "appendバッファがオーバーフローしました(counter={appended}, capacity={CAPACITY})"
        ));
    }
    // 健全性ガード（W-1相当）: 全チャンク累積の appended は u32 桁溢れの主要な発生源
    // （n=300・slot=5では全組み合わせ数が u32::MAX の約4.5倍に達する）。ラップアラウンド後の
    // 値が `CAPACITY` 以下に着地すると上のガードをすり抜けるため、accept 数が物理的に
    // 超えられない `total_combinations` との整合を追加でチェックする。
    if u64::from(appended) > total_combinations {
        return Err(format!(
            "appendedカウンタが理論上限を超えています(appended={appended}, \
             total_combinations={total_combinations})。u32カウンタの桁溢れの可能性があります"
        ));
    }

    let mut topk = TopK::new(top_k);
    if appended > 0 {
        let combos = read_combos(&ctx.device, &ctx.queue, &out_buf, appended as usize)?;
        for raw in combos.chunks_exact(5) {
            let (key, combo) = recompute_combo(prepared, slot_count, raw, mode)?;
            topk.offer(key, &combo);
        }
    }

    let mut ranked = topk.into_vec();
    ranked.sort_by(|a, b| b.cmp(a));
    ranked.truncate(top_k);

    let pruned_pct = if prefix_count > 0 {
        100.0 * (1.0 - f64::from(survivor_total) / f64::from(prefix_count))
    } else {
        0.0
    };
    log::info!(
        "[gpu-chunked] n={n} slot={slot_count} top_k={top_k} combos={total_combinations} \
         seed_solutions={} appended={appended} prefixes={prefix_count} chunks={num_chunks} \
         survivors={survivor_total} pruned_pct={pruned_pct:.1}% gpu={gpu_elapsed:?} total={:?}",
        seed_ranked.len(),
        run_start.elapsed()
    );

    Ok(ranked)
}

/// GPU探索本体。失敗時は Err（呼び出し側の [`optimize`] が CPU フォールバックする）。
/// `prune_enabled`: プレフィックス単位枝刈り（Phase B）の有効/無効。取りこぼし防止テスト
/// （枝刈りON/OFF一致検証）用に外から切り替えられるようにしている（本番は常に true）。
fn run_gpu_search(
    ctx: &GpuContext,
    prepared: &Prepared,
    top_k: usize,
    slot_count: usize,
    mode: RankMode,
    prune_enabled: bool,
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
    let seed_ranked = optimizer::search_cpu(&seed_prepared, top_k, slot_count, mode, true, true);

    let mut topk = TopK::new(top_k);

    let threshold: (u32, u32) = if seed_ranked.len() >= top_k {
        pack_key(&seed_ranked[top_k - 1].key, mode)
    } else {
        // シード部分集合だけでは top_k 件すら見つからない稀なケース。安全側に倒し、
        // 閾値なし（=すべて拾う）から始める。
        (0, 0)
    };

    // アップロードするデータ（この関数呼び出し内では不変）。
    let binom_bytes = build_binom_table(n, slot_count)?;
    let cand_bytes = build_cand_parts(&prepared.cands);
    let req_bytes = build_req_entries(&prepared.required_idxs);
    let suffix_max_bytes = build_suffix_max_bytes(prepared);
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
    let suffix_max_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suffix_max"),
            contents: &suffix_max_bytes,
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
    // counters[0]=appended件数, counters[1]=pruned プレフィックス件数（array<atomic<u32>, 2>）。
    // 新規バインディングを増やさず既存の counter バッファを2要素へ拡張して収める。
    let counter_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("counters"),
        size: 8,
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
    // counters[0]=appended, counters[1]=pruned をともに0初期化。
    ctx.queue
        .write_buffer(&counter_buf, 0, &[0u8; 8]);

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
        u32::from(prune_enabled),
        rank_mode_u32(mode),
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
            wgpu::BindGroupEntry {
                binding: 6,
                resource: suffix_max_buf.as_entire_binding(),
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

    let counter_value = read_u32_at(&ctx.device, &ctx.queue, &counter_buf, 0)?;
    let pruned_count = read_u32_at(&ctx.device, &ctx.queue, &counter_buf, 4)?;
    let gpu_elapsed = dispatch_start.elapsed();

    // オーバーフロー（counter > 容量）はそのままCPUフォールバックへ回す。atomicAdd の到達順は
    // 実行のたびに変わりうる任意サブセットであり、閾値以上の中の「上位」を保証しないため、
    // 部分出力から閾値を引き締めて再試行するのは不健全（真の解を取りこぼしうる）。
    if counter_value > CAPACITY {
        return Err(format!(
            "appendバッファがオーバーフローしました(counter={counter_value}, capacity={CAPACITY})"
        ));
    }
    // 健全性ガード（W-1相当、chunked側と同じ理由）: counters[0] は u32 のため、accept 数が
    // u32::MAX 近くに達するとラップアラウンドし、上の `> CAPACITY` 比較を偽陰性ですり抜け
    // うる。accept 数は全組み合わせ数 `total_combinations` を物理的に超えられないため、
    // ここでの矛盾はラップアラウンドの動かぬ証拠になる（完全な検知ではなく追加ガード）。
    if u64::from(counter_value) > total_combinations {
        return Err(format!(
            "appendedカウンタが理論上限を超えています(counter={counter_value}, \
             total_combinations={total_combinations})。u32カウンタの桁溢れの可能性があります"
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
            let key = acc.key(mode);
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

    let pruned_pct = if prefix_count > 0 {
        100.0 * f64::from(pruned_count) / f64::from(prefix_count)
    } else {
        0.0
    };
    log::info!(
        "[gpu] n={n} slot={slot_count} top_k={top_k} combos={total_combinations} \
         seed_solutions={} appended={appended} prefixes={prefix_count} \
         pruned={pruned_count}({pruned_pct:.1}%) prune_enabled={prune_enabled} \
         gpu={gpu_elapsed:?} total={:?}",
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
    mode: RankMode,
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
        mode,
    )
}

/// GPU探索の公開API。シグネチャ・結果は [`crate::optimizer::optimize`] と同一。
/// どんな失敗（デバイス初期化失敗・値域超過・実行時エラー/panic）でもユーザーへエラーを
/// 返さず、CPU探索（[`crate::optimizer::optimize`]）へ委譲して完遂する。
/// 本番は常に2フェーズ実装（[`GpuVariant::Chunked`]、[`optimize_with_opts`] 参照）を使う。
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
        GpuVariant::Chunked,
    )
}

/// `optimize` の実体。`variant` は使用するGPU実装を切り替える（[`GpuVariant`] 参照。
/// [`optimizer::optimize_with_opts`] の requirements/B&B剪定トグルと同じ設計思想。取りこぼし
/// 防止テストで複数の独立実装の結果一致を検証するために分離している。本番の [`optimize`] は
/// 常に [`GpuVariant::Chunked`]）。
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
    variant: GpuVariant,
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
        // GPU探索を実行していない（探索するまでもなく空）ため engine は "cpu" 扱い。
        return Ok(optimizer::assemble(&prepared, Vec::new(), "cpu"));
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
    } else if prepared
        .cands
        .iter()
        .flat_map(|c| c.parts.iter())
        .any(|&(_, v)| v < 0)
    {
        // eval_link は Link モードでは hi の下位14bitに、Lv5 モードでは lo の上位16bitに
        // 直接詰まるため、負値が1つでも混ざると符号ビットの桁上がりで他の成分（Link では
        // sel_present/sel_lv6/lv6、Lv5 では excl）まで汚染しうる。value_bounds は上界しか
        // 見ないため負値をすり抜けさせてしまう。ゲーム側の属性値は常に非負のはずだが、将来
        // データが崩れた場合の安全弁として明示的に弾く。
        Some("属性値に負値が含まれています（キー packing は非負値を前提とする）".to_string())
    } else {
        let (eval_bound, excl_bound) = value_bounds(&prepared, slot_count);
        let eval_max = eval_link_key_max(mode);
        if eval_bound > eval_max || excl_bound > EXCL_KEY_MAX {
            Some(format!(
                "eval_link/excl の理論上界がキーpacking幅を超えています\
                 (mode={mode:?}, eval={eval_bound}>{eval_max:#x}, excl={excl_bound}>{EXCL_KEY_MAX:#x})"
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
            mode,
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
                mode,
                &format!("GPU初期化に失敗しました: {e}"),
            );
        }
    };

    // GPU実行中のpanic（デバイスロスト等、wgpuはResultではなくpanicで報告することがある）も
    // 捕捉してCPUフォールバックへ回す。パニック後にGPUステート自体を再利用しないため
    // AssertUnwindSafe で安全。
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match variant {
        GpuVariant::Chunked => run_gpu_search_chunked(ctx, &prepared, top_k, slot_count, mode),
        GpuVariant::SinglePassPruned => {
            run_gpu_search(ctx, &prepared, top_k, slot_count, mode, true)
        }
        GpuVariant::SinglePassUnpruned => {
            run_gpu_search(ctx, &prepared, top_k, slot_count, mode, false)
        }
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
                mode,
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
                mode,
                "GPU実行中にpanicが発生しました",
            );
        }
    };

    // GPU探索がマージまで完遂した場合のみ "gpu"。
    Ok(optimizer::assemble(&prepared, ranked, "gpu"))
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
    /// [`RankMode::Link`]/[`RankMode::Lv5`] の両方でループする（呼び出し側の引数追加なしで
    /// 既存の全呼び出し箇所を自動的に両モードでカバーする設計。案A実装時に導入）。
    #[allow(clippy::too_many_arguments)]
    fn assert_gpu_matches_cpu(
        modules: &[Module],
        selected_ids: &[i32],
        hard_exclude_ids: &[i32],
        soft_exclude_ids: &[i32],
        requirements: &[(i32, usize)],
        top_k: usize,
        slot_count: usize,
        ctx_label: &str,
    ) {
        for mode in [RankMode::Link, RankMode::Lv5] {
            let ctx_label = format!("{ctx_label} mode={mode:?}");
            let cpu = optimizer::optimize(
                modules,
                selected_ids,
                Some("all"),
                hard_exclude_ids,
                soft_exclude_ids,
                requirements,
                top_k,
                slot_count,
                mode,
            )
            .unwrap_or_else(|e| panic!("CPU optimize failed [{ctx_label}]: {e}"));

            let t = Instant::now();
            let gpu = optimize(
                modules,
                selected_ids,
                Some("all"),
                hard_exclude_ids,
                soft_exclude_ids,
                requirements,
                top_k,
                slot_count,
                mode,
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
            }
        }
    }

    /// [`merge_topk_for_refinement`] の重複除去を検証する（C-1回帰テスト、GPU不要）。
    /// seed と extra に同一 combo が重複して存在する場合、dedup した distinct top-k の
    /// k位を返すことを確認する。dedup しない実装（修正前の旧コード）では、この入力で
    /// worst_key が真の3位(85)ではなく90になる（真に異なる80のcomboが誤って追い出され、
    /// 締まりすぎた admissible 違反の閾値が返る）。
    #[test]
    fn merge_topk_for_refinement_dedups_combos() {
        // Link モードの並び (sel_present, sel_lv6, lv6, eval_link, lv5, -excl) を模す。
        // このテストは merge_topk_for_refinement の重複除去ロジックのみを検証しており、
        // Key の並び順（モード）には依存しない。
        fn key(eval_link: i64) -> Key {
            [0, 0, 0, eval_link, 0, 0]
        }

        let seed_ranked = vec![
            Ranked {
                key: key(100),
                combo: vec![1],
            },
            Ranked {
                key: key(90),
                combo: vec![2],
            },
            Ranked {
                key: key(80),
                combo: vec![3],
            },
        ];
        // extra[0] は seed[0] と同一 combo（GPU 先頭チャンクで「再発見」された重複を想定）。
        // extra[1] は真に新規の combo。
        let extra = vec![(key(100), vec![1u32]), (key(85), vec![4u32])];

        let worst = merge_topk_for_refinement(&seed_ranked, extra, 3);
        // distinct(seed ∪ extra) = {100, 90, 85, 80} の上位3件は {100, 90, 85}。3位=85。
        assert_eq!(
            worst,
            Some(key(85)),
            "dedup後のk位は真のdistinct top-3の3位(85)と一致するべき"
        );
    }

    /// GPU実装バリアント同士（[`GpuVariant`]）の結果が完全一致することを検証する
    /// （取りこぼし検出の最強テスト。CPU参照不要で高速に回せる）。Phase B2 では
    /// [`GpuVariant::Chunked`]（本番・2フェーズ）と [`GpuVariant::SinglePassUnpruned`]
    /// （枝刈りロジックを一切持たない最も単純な独立実装、基準として使う）を比較するのが
    /// 主力。片方が取りこぼしを起こしていれば、もう片方より劣化した結果になるはず
    /// （[`crate::optimizer::tests::bnb_pruning_on_off_same_keys`] と同じ設計思想）。
    /// [`RankMode::Link`]/[`RankMode::Lv5`] の両方でループする（[`assert_gpu_matches_cpu`]
    /// と同じ設計）。
    #[allow(clippy::too_many_arguments)]
    fn assert_variant_matches(
        modules: &[Module],
        selected_ids: &[i32],
        soft_exclude_ids: &[i32],
        requirements: &[(i32, usize)],
        top_k: usize,
        slot_count: usize,
        variant_a: GpuVariant,
        variant_b: GpuVariant,
        ctx_label: &str,
    ) {
        for mode in [RankMode::Link, RankMode::Lv5] {
            let ctx_label = format!("{ctx_label} mode={mode:?}");
            let a = optimize_with_opts(
                modules,
                selected_ids,
                Some("all"),
                &[],
                soft_exclude_ids,
                requirements,
                top_k,
                slot_count,
                mode,
                variant_a,
            )
            .unwrap_or_else(|e| panic!("{variant_a:?} failed [{ctx_label}]: {e}"));
            let b = optimize_with_opts(
                modules,
                selected_ids,
                Some("all"),
                &[],
                soft_exclude_ids,
                requirements,
                top_k,
                slot_count,
                mode,
                variant_b,
            )
            .unwrap_or_else(|e| panic!("{variant_b:?} failed [{ctx_label}]: {e}"));

            assert_eq!(
                a.solutions.len(),
                b.solutions.len(),
                "solutions件数 mismatch [{ctx_label}]"
            );
            for (i, (x, y)) in a.solutions.iter().zip(b.solutions.iter()).enumerate() {
                let ctx = format!("{ctx_label} rank={i}");
                let x_keys: Vec<i64> = x.modules.iter().map(|m| m.key).collect();
                let y_keys: Vec<i64> = y.modules.iter().map(|m| m.key).collect();
                assert_eq!(x_keys, y_keys, "module key列 mismatch [{ctx}]");
                assert_eq!(x.link_effect, y.link_effect, "link_effect mismatch [{ctx}]");
                assert_eq!(x.eval_link, y.eval_link, "eval_link mismatch [{ctx}]");
                assert_eq!(x.lv6_count, y.lv6_count, "lv6_count mismatch [{ctx}]");
                assert_eq!(x.lv5_count, y.lv5_count, "lv5_count mismatch [{ctx}]");
                assert_eq!(
                    x.selected_lv6, y.selected_lv6,
                    "selected_lv6 mismatch [{ctx}]"
                );
                assert_eq!(
                    x.selected_present, y.selected_present,
                    "selected_present mismatch [{ctx}]"
                );
            }
        }
    }

    /// C-1回帰テスト: 閾値リファインメント（[`merge_topk_for_refinement`]）で CPU シード
    /// top-k の combo が GPU 先頭チャンクで実際に「再発見」される条件（n=200・
    /// prefix_count が [`REFINE_CHUNKS`] 個ぶんの CHUNK_SIZE を超え、かつ top_k を
    /// 上限(64)近くまで大きくして先頭チャンクでの新規発見数を増やす）で、Chunked
    /// （リファインメントを行う本番経路）と SinglePassUnpruned（リファインメント無しの
    /// 単純な独立実装、真の基準）の結果が完全一致することを検証する。
    ///
    /// 実測メモ（このテスト作成時点）: n=200 slot5 top_k=64 で appended0=93（シードの
    /// seed_solutions=64 のほぼ全てが先頭チャンクで再発見される規模）となり、dedup を
    /// 意図的に無効化すると閾値リファインメント結果の数値（`old=...` → `new=...` の
    /// ログ）が dedup 有無で確かに変わることを確認した（[`merge_topk_for_refinement`]
    /// のバグが実際にこの経路で発火することの実証）。ただし、この規模のデータでは
    /// dedup 無効化でも Chunked/SinglePassUnpruned の最終結果（top-k）までの不一致は
    /// 再現できなかった（締まった閾値でも真の取りこぼしには至らなかった）。取りこぼしは
    /// TopK 内で押し出されたキーが真の k位のすぐ下に位置する必要があり、確率的に狭い
    /// window でしか発生しないため。本テストはそれでも「重複 offer が実際に起きる条件」
    /// を固定した回帰テストとして、[`merge_topk_for_refinement`] の dedup ロジックが
    /// 今後のリファクタで壊れた場合に検出できる状態を維持する。
    /// BPSR_MODULE_DUMP_200 必須（scratchpad の gen_modules.py で n=200 を生成）。
    /// 実GPU必須のため #[ignore]。
    #[test]
    #[ignore]
    fn gpu_chunked_matches_single_pass_refinement_dedup() {
        let _ = env_logger::builder().is_test(true).try_init();

        let dump_path = std::env::var("BPSR_MODULE_DUMP_200")
            .expect("BPSR_MODULE_DUMP_200 を設定してください（n=200データ、gen_modules.py参照）");
        let modules = load_dump(&dump_path);
        for &slot in &[4usize, 5usize] {
            for &top_k in &[16usize, 32, 48, 64] {
                let ctx = format!("refine-dedup slot{slot} k{top_k}");
                assert_variant_matches(
                    &modules,
                    &[],
                    &[],
                    &[],
                    top_k,
                    slot,
                    GpuVariant::Chunked,
                    GpuVariant::SinglePassUnpruned,
                    &ctx,
                );
            }
        }
    }

    /// mods_230/260/280/300（BPSR_MODULE_DUMP_230/260/280/300）× slot4/5 × top_k{3,10} で
    /// 2フェーズ（本番）と単一パス・枝刈り無効（最も単純な独立実装）の結果一致を検証する。
    /// 実GPU必須のため #[ignore]。
    #[test]
    #[ignore]
    fn gpu_chunked_matches_single_pass() {
        let _ = env_logger::builder().is_test(true).try_init();

        let mut datasets: Vec<(&str, Vec<Module>)> = Vec::new();
        for (label, var) in [
            ("n230", "BPSR_MODULE_DUMP_230"),
            ("n260", "BPSR_MODULE_DUMP_260"),
            ("n280", "BPSR_MODULE_DUMP_280"),
            ("n300", "BPSR_MODULE_DUMP_300"),
        ] {
            if let Ok(path) = std::env::var(var) {
                datasets.push((label, load_dump(&path)));
            }
        }
        assert!(
            !datasets.is_empty(),
            "BPSR_MODULE_DUMP_230/260/280/300 のいずれかを設定してください"
        );

        // 目標属性・ソフト除外・requirements も real142 テストと同じ組合せで確認する。
        for (label, modules) in &datasets {
            for &slot in &[4usize, 5usize] {
                for &top_k in &[3usize, 10] {
                    let ctx = format!("{label} slot{slot} k{top_k} plain");
                    assert_variant_matches(
                        modules,
                        &[],
                        &[],
                        &[],
                        top_k,
                        slot,
                        GpuVariant::Chunked,
                        GpuVariant::SinglePassUnpruned,
                        &ctx,
                    );

                    let ctx = format!("{label} slot{slot} k{top_k} selected");
                    assert_variant_matches(
                        modules,
                        &[2104],
                        &[],
                        &[],
                        top_k,
                        slot,
                        GpuVariant::Chunked,
                        GpuVariant::SinglePassUnpruned,
                        &ctx,
                    );

                    let ctx = format!("{label} slot{slot} k{top_k} soft_exclude");
                    assert_variant_matches(
                        modules,
                        &[2104],
                        &[1113],
                        &[],
                        top_k,
                        slot,
                        GpuVariant::Chunked,
                        GpuVariant::SinglePassUnpruned,
                        &ctx,
                    );

                    let ctx = format!("{label} slot{slot} k{top_k} requirements");
                    assert_variant_matches(
                        modules,
                        &[2104],
                        &[],
                        &[(1110, 1)],
                        top_k,
                        slot,
                        GpuVariant::Chunked,
                        GpuVariant::SinglePassUnpruned,
                        &ctx,
                    );
                }
            }
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
                    assert_gpu_matches_cpu(modules, &[], &[], &[], &[], top_k, slot, &ctx);

                    let ctx = format!("{label} slot{slot} k{top_k} selected");
                    assert_gpu_matches_cpu(modules, &[2104], &[], &[], &[], top_k, slot, &ctx);

                    let ctx = format!("{label} slot{slot} k{top_k} soft_exclude");
                    assert_gpu_matches_cpu(modules, &[2104], &[], &[1113], &[], top_k, slot, &ctx);

                    let ctx = format!("{label} slot{slot} k{top_k} requirements");
                    assert_gpu_matches_cpu(
                        modules,
                        &[2104],
                        &[],
                        &[],
                        &[(1110, 1)],
                        top_k,
                        slot,
                        &ctx,
                    );
                }
            }
        }
    }

    /// 実データ(real142)で3つの独立GPU実装（[`GpuVariant::Chunked`]/
    /// [`GpuVariant::SinglePassPruned`]/[`GpuVariant::SinglePassUnpruned`]）が全て一致する
    /// ことを確認する。SinglePassPruned（Phase Bの枝刈り単一パス実装）は本番経路からは
    /// 外れたが、デバッグ/比較用の独立実装として引き続き有用なため、Chunked との相互比較
    /// にも使う（[`gpu_chunked_matches_single_pass`] は SinglePassUnpruned のみと比較する
    /// ため、これで3実装すべての組合せをカバーする）。実GPU必須のため #[ignore]。
    #[test]
    #[ignore]
    fn gpu_all_variants_match_real142() {
        let _ = env_logger::builder().is_test(true).try_init();
        let dump_path = std::env::var("BPSR_MODULE_DUMP")
            .unwrap_or_else(|_| "../../extracted_game_data/owned_modules.json".to_string());
        let modules = load_dump(&dump_path);
        for &slot in &[4usize, 5usize] {
            let ctx = format!("real142 slot{slot} k10 chunked-vs-single_pass_pruned");
            assert_variant_matches(
                &modules,
                &[2104],
                &[],
                &[],
                10,
                slot,
                GpuVariant::Chunked,
                GpuVariant::SinglePassPruned,
                &ctx,
            );
            let ctx = format!("real142 slot{slot} k10 single_pass_pruned-vs-unpruned");
            assert_variant_matches(
                &modules,
                &[2104],
                &[],
                &[],
                10,
                slot,
                GpuVariant::SinglePassPruned,
                GpuVariant::SinglePassUnpruned,
                &ctx,
            );
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
        assert_gpu_matches_cpu(&modules, &[], &[], &[], &[], 10, 4, "tie_break slot4");
        assert_gpu_matches_cpu(&modules, &[], &[], &[], &[], 10, 5, "tie_break slot5");
    }

    /// n=300（MAX_N上限付近）で、Phase C要求の条件バリエーション（各1ケース以上）が
    /// CPU参照と完全一致することを検証する: 下限Lv指定あり / ソフト除外 / ハード除外 /
    /// 目標属性なし / 目標属性がプール中に存在しない（selected_present=0になる）。
    /// BPSR_MODULE_DUMP_300 必須。実GPU必須のため #[ignore]。
    #[test]
    #[ignore]
    fn gpu_matches_cpu_condition_variations_n300() {
        let _ = env_logger::builder().is_test(true).try_init();
        let path = std::env::var("BPSR_MODULE_DUMP_300").expect("BPSR_MODULE_DUMP_300が必要");
        let modules = load_dump(&path);

        // mods_300.json に実在する属性: 1110/1113/1307/1308/1407/1408/1409/1410 等
        // （gen_modules.py で実データを複製したデータセットのため、実データ由来の属性IDが
        // そのまま含まれる）。999999 はどの候補にも存在しない架空の属性ID。
        for &slot in &[4usize, 5usize] {
            assert_gpu_matches_cpu(
                &modules,
                &[],
                &[],
                &[],
                &[],
                10,
                slot,
                &format!("n300 slot{slot} no_target"),
            );
            assert_gpu_matches_cpu(
                &modules,
                &[1408],
                &[],
                &[],
                &[(1110, 2)],
                10,
                slot,
                &format!("n300 slot{slot} requirement_lv2"),
            );
            assert_gpu_matches_cpu(
                &modules,
                &[1408],
                &[],
                &[1113],
                &[],
                10,
                slot,
                &format!("n300 slot{slot} soft_exclude"),
            );
            assert_gpu_matches_cpu(
                &modules,
                &[1408],
                &[1409],
                &[],
                &[],
                10,
                slot,
                &format!("n300 slot{slot} hard_exclude"),
            );
            assert_gpu_matches_cpu(
                &modules,
                &[999_999],
                &[],
                &[],
                &[],
                10,
                slot,
                &format!("n300 slot{slot} target_not_in_pool"),
            );
        }
    }

    /// 境界値: 候補数==slots（解1通りのみ）/ 候補数<slots（trivially_empty）で
    /// GPU/CPU が完全一致することを検証する。実GPU必須のため #[ignore]。
    #[test]
    #[ignore]
    fn gpu_matches_cpu_candidate_count_boundaries() {
        let _ = env_logger::builder().is_test(true).try_init();

        // 候補数 == slot_count(5): C(5,5)=1通りのみ。
        let exact = vec![
            module(1, 5500103, &[(1, 5)]),
            module(2, 5500103, &[(1, 5)]),
            module(3, 5500103, &[(1, 5)]),
            module(4, 5500103, &[(1, 5)]),
            module(5, 5500103, &[(1, 5)]),
        ];
        assert_gpu_matches_cpu(&exact, &[], &[], &[], &[], 10, 5, "candidates==slots(5)");

        // 候補数 < slot_count(5): 解なし（trivially_empty）。
        let too_few = vec![
            module(1, 5500103, &[(1, 5)]),
            module(2, 5500103, &[(1, 5)]),
            module(3, 5500103, &[(1, 5)]),
            module(4, 5500103, &[(1, 5)]),
        ];
        assert_gpu_matches_cpu(&too_few, &[], &[], &[], &[], 10, 5, "candidates<slots(4<5)");
    }

    /// 境界: n=301（GPU の [`MAX_N`]=300 を1件超える）で CPU フォールバックが維持される
    /// こと（`engine=="cpu"`）と、結果自体が CPU 直接呼び出しと一致することを検証する。
    /// BPSR_MODULE_DUMP_301 必須。実GPU必須のため #[ignore]。
    #[test]
    #[ignore]
    fn gpu_falls_back_to_cpu_at_n301() {
        let _ = env_logger::builder().is_test(true).try_init();
        let path = std::env::var("BPSR_MODULE_DUMP_301").expect("BPSR_MODULE_DUMP_301が必要");
        let modules = load_dump(&path);
        assert_eq!(modules.len(), 301, "データセットは301件である必要がある");

        for mode in [RankMode::Link, RankMode::Lv5] {
            let gpu = optimize(&modules, &[], Some("all"), &[], &[], &[], 10, 5, mode)
                .expect("optimize failed");
            assert_eq!(
                gpu.engine, "cpu",
                "n=301はMAX_N=300を超えるためCPUフォールバックするはず mode={mode:?}"
            );

            let cpu = optimizer::optimize(&modules, &[], Some("all"), &[], &[], &[], 10, 5, mode)
                .expect("CPU optimize failed");
            assert_eq!(
                cpu.solutions.len(),
                gpu.solutions.len(),
                "solutions件数 mismatch mode={mode:?}"
            );
            for (i, (c, g)) in cpu.solutions.iter().zip(gpu.solutions.iter()).enumerate() {
                let c_keys: Vec<i64> = c.modules.iter().map(|m| m.key).collect();
                let g_keys: Vec<i64> = g.modules.iter().map(|m| m.key).collect();
                assert_eq!(
                    c_keys, g_keys,
                    "module key列 mismatch mode={mode:?} rank={i}"
                );
            }
        }
    }

    /// 閾値境界データ: 同一属性構成の複製モジュールを大量に含むデータセット
    /// （BPSR_MODULE_DUMP_300_DUP、gen_modules.py の dup_count 拡張で生成）で、シードの
    /// k位キーと同点の組合せが多数ある状況でも GPU/CPU が完全一致することを検証する
    /// （accept 判定の `>=`/`>` 取り違いはこのようなデータで最も顕在化しやすい）。
    /// 実GPU必須のため #[ignore]。
    #[test]
    #[ignore]
    fn gpu_matches_cpu_threshold_boundary_duplicates() {
        let _ = env_logger::builder().is_test(true).try_init();
        let path =
            std::env::var("BPSR_MODULE_DUMP_300_DUP").expect("BPSR_MODULE_DUMP_300_DUPが必要");
        let modules = load_dump(&path);
        for &slot in &[4usize, 5usize] {
            for &top_k in &[3usize, 10] {
                assert_gpu_matches_cpu(
                    &modules,
                    &[],
                    &[],
                    &[],
                    &[],
                    top_k,
                    slot,
                    &format!("n300dup slot{slot} k{top_k}"),
                );
            }
        }
    }
}
