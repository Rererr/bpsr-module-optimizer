/**
 * owned_modules.json を実データからブートストラップで水増しする開発用スクリプト。
 *
 *   node scripts/synthesize_dump.js <入力dump> <出力dump> <目標件数> [seed]
 *
 * 目的は性能検証（n≥200 の探索時間計測）と UI デモ用の盤面づくり。実データの
 * 分布から再サンプリングするため、観測されたゲーム上のルールを自動的に満たす。
 *
 * 入力データから読み取ったルール（166件・例外なし）:
 *   - 品質と部位数が 1:1 対応（q2=1個 / q3=2個 / q4=3個）
 *   - config_id がカテゴリと品質を決める（5500{1=攻撃,2=支援,3=防御}{01=基本,02=高性能,03=卓越,04=EXC精選}）
 *   - 同一モジュール内に同じ属性は入らない
 *   - 極属性（2xxx）は q4 のみ・1モジュールにつき最大1個
 *   - カテゴリごとに使える通常属性が決まっており、集中系(1407-1410)のみ全カテゴリ共通
 *
 * 値は「同品質の実モジュールが持つ値の組」をそのまま流用する。これにより
 * 部位ごとの値分布とモジュール内合計の上限（q3≤14 / q4≤20）が実データを超えない。
 */

import fs from "node:fs";
import process from "node:process";

// 極属性のカテゴリ対応。ID の上2桁が通常属性のグループ番号と対応する
// （21xx↔11xx=攻撃系, 22xx↔12xx=支援系, 23xx↔13xx=防御系, 24xx↔14xx=集中系＝全カテゴリ共通）。
// 実データで観測された 2205=支援 / 2304=防御 / 2404,2405=防御 / 2405=攻撃 / 2406=支援 は
// すべてこの規則と矛盾しない。未観測の 2104/2105/2204 もこの規則で割り当てる。
const ULTRA_CATEGORIES = {
  2104: ["attack"],
  2105: ["attack"],
  2204: ["support"],
  2205: ["support"],
  2304: ["guardian"],
  2404: ["attack", "support", "guardian"],
  2405: ["attack", "support", "guardian"],
  2406: ["attack", "support", "guardian"],
};
const isUltra = (attrId) => attrId >= 2000;

const CATEGORY_OF = { 1: "attack", 2: "support", 3: "guardian" };
const categoryOf = (configId) => CATEGORY_OF[Math.floor((configId % 10000) / 100)];

// 再現性のため固定シードの xorshift32 を使う（ベンチマークの入力が実行ごとに変わらないように）。
function makeRng(seed) {
  let s = seed >>> 0 || 1;
  return () => {
    s ^= s << 13;
    s >>>= 0;
    s ^= s >> 17;
    s ^= s << 5;
    s >>>= 0;
    return s / 0x100000000;
  };
}

function pick(rng, items) {
  return items[Math.floor(rng() * items.length)];
}

/** 重み付き抽選。weights は items と同じ長さの正の数値配列。 */
function pickWeighted(rng, items, weights) {
  const total = weights.reduce((a, b) => a + b, 0);
  let r = rng() * total;
  for (let i = 0; i < items.length; i++) {
    r -= weights[i];
    if (r <= 0) return items[i];
  }
  return items[items.length - 1];
}

function main() {
  const [srcPath, outPath, targetArg, seedArg] = process.argv.slice(2);
  if (!srcPath || !outPath || !targetArg) {
    console.error(
      "usage: node scripts/synthesize_dump.js <入力dump> <出力dump> <目標件数> [seed]",
    );
    process.exit(1);
  }
  const target = Number(targetArg);
  if (!Number.isInteger(target) || target <= 0) {
    console.error(`目標件数が不正です: ${targetArg}`);
    process.exit(1);
  }

  const base = JSON.parse(fs.readFileSync(srcPath, "utf8"));
  if (!Array.isArray(base) || base.length === 0) {
    console.error(`入力dumpが空か配列ではありません: ${srcPath}`);
    process.exit(1);
  }
  if (target < base.length) {
    console.error(`目標件数 ${target} が入力の ${base.length} 件を下回っています（水増し専用）`);
    process.exit(1);
  }

  const rng = makeRng(seedArg ? Number(seedArg) : 20260725);

  // ---- 実データから分布を抽出 ----
  const configCount = new Map(); // config_id -> 件数
  const nameOfConfig = new Map(); // config_id -> ダンプ上の名称
  const qualityOfConfig = new Map(); // config_id -> 品質
  const partsOfQuality = new Map(); // quality -> 部位数（例外があれば検出する）
  const valueSetsByQuality = new Map(); // quality -> 値の組の配列
  const normalAttrWeight = new Map(); // `${category}:${attr_id}` -> 出現回数
  const ultraCountByQuality = new Map(); // quality -> 極属性を持つモジュール数
  const moduleCountByQuality = new Map();
  const attrNames = new Map();

  for (const m of base) {
    const cat = categoryOf(m.config_id);
    if (!cat) {
      console.error(`未知の config_id: ${m.config_id}`);
      process.exit(1);
    }
    configCount.set(m.config_id, (configCount.get(m.config_id) ?? 0) + 1);
    nameOfConfig.set(m.config_id, m.name);
    qualityOfConfig.set(m.config_id, m.quality);

    const prev = partsOfQuality.get(m.quality);
    if (prev !== undefined && prev !== m.parts.length) {
      console.error(
        `品質と部位数の対応が一意でありません（q${m.quality}: ${prev} と ${m.parts.length}）。` +
          "ルール前提が崩れているため生成を中止します。",
      );
      process.exit(1);
    }
    partsOfQuality.set(m.quality, m.parts.length);

    if (!valueSetsByQuality.has(m.quality)) valueSetsByQuality.set(m.quality, []);
    valueSetsByQuality.get(m.quality).push(m.parts.map((p) => p.value));

    moduleCountByQuality.set(m.quality, (moduleCountByQuality.get(m.quality) ?? 0) + 1);
    if (m.parts.some((p) => isUltra(p.attr_id)))
      ultraCountByQuality.set(m.quality, (ultraCountByQuality.get(m.quality) ?? 0) + 1);

    for (const p of m.parts) {
      attrNames.set(p.attr_id, p.attr_name);
      if (!isUltra(p.attr_id)) {
        const k = `${cat}:${p.attr_id}`;
        normalAttrWeight.set(k, (normalAttrWeight.get(k) ?? 0) + 1);
      }
    }
  }

  // カテゴリごとの通常属性プール（実データに出た属性のみ＝観測外の組合せを作らない）。
  const normalPool = new Map(); // category -> [{attr_id, weight}]
  for (const [k, w] of normalAttrWeight) {
    const [cat, id] = k.split(":");
    if (!normalPool.has(cat)) normalPool.set(cat, []);
    normalPool.get(cat).push({ attrId: Number(id), weight: w });
  }

  // 極属性プール。未観測の属性も出現させるため Laplace 平滑化（観測回数+1）を使う。
  const ultraObserved = new Map();
  for (const m of base)
    for (const p of m.parts)
      if (isUltra(p.attr_id)) ultraObserved.set(p.attr_id, (ultraObserved.get(p.attr_id) ?? 0) + 1);
  const ultraPool = new Map(); // category -> [{attrId, weight}]
  for (const [idStr, cats] of Object.entries(ULTRA_CATEGORIES)) {
    const id = Number(idStr);
    for (const cat of cats) {
      if (!ultraPool.has(cat)) ultraPool.set(cat, []);
      ultraPool.get(cat).push({ attrId: id, weight: (ultraObserved.get(id) ?? 0) + 1 });
    }
  }

  // 極属性の出現率（品質別）。実データでは q4 のみ。
  const ultraRate = new Map();
  for (const [q, n] of moduleCountByQuality)
    ultraRate.set(q, (ultraCountByQuality.get(q) ?? 0) / n);

  // ---- 生成 ----
  const usedKeys = new Set(base.map((m) => m.key));
  const configIds = [...configCount.keys()];
  const configWeights = configIds.map((c) => configCount.get(c));
  // 極属性の名称は実データに無いものがあるため、判明分＋既知IDの対応表で補う。
  const ULTRA_NAMES = {
    2104: "極・ダメージ増強",
    2105: "極・適応力",
    2204: "極・HP凝縮",
    2205: "極・応急処置",
    2304: "極・絶境守護",
    2404: "極・HP変動",
    2405: "極・HP吸収",
    2406: "極・幸運会心",
  };

  const generated = [];
  const need = target - base.length;
  for (let i = 0; i < need; i++) {
    const configId = pickWeighted(rng, configIds, configWeights);
    const category = categoryOf(configId);
    const quality = qualityOfConfig.get(configId);
    const partCount = partsOfQuality.get(quality);

    // 属性の組を作る（重複なし）。極属性は品質別の実測率で最大1個だけ混ぜる。
    const attrIds = [];
    if (rng() < (ultraRate.get(quality) ?? 0)) {
      const pool = ultraPool.get(category) ?? [];
      if (pool.length > 0)
        attrIds.push(
          pickWeighted(
            rng,
            pool.map((x) => x.attrId),
            pool.map((x) => x.weight),
          ),
        );
    }
    const pool = normalPool.get(category);
    let guard = 0;
    while (attrIds.length < partCount) {
      const id = pickWeighted(
        rng,
        pool.map((x) => x.attrId),
        pool.map((x) => x.weight),
      );
      if (!attrIds.includes(id)) attrIds.push(id);
      if (++guard > 1000) {
        console.error(`カテゴリ ${category} の属性プールが部位数 ${partCount} に足りません`);
        process.exit(1);
      }
    }

    // 値は同品質の実モジュールの「値の組」をそのまま借り、属性への割当だけシャッフルする。
    const values = [...pick(rng, valueSetsByQuality.get(quality))];
    for (let j = values.length - 1; j > 0; j--) {
      const k = Math.floor(rng() * (j + 1));
      [values[j], values[k]] = [values[k], values[j]];
    }

    let key;
    do {
      key = 2782 + Math.floor(rng() * 22000);
    } while (usedKeys.has(key));
    usedKeys.add(key);

    generated.push({
      key,
      uuid: key, // 実データは全件 key === uuid。
      config_id: configId,
      name: nameOfConfig.get(configId),
      category,
      quality,
      parts: attrIds.map((attrId, idx) => ({
        attr_id: attrId,
        attr_name: attrNames.get(attrId) ?? ULTRA_NAMES[attrId] ?? `#${attrId}`,
        value: values[idx],
      })),
    });
  }

  // 実データに1件も無い極属性（2104/2105/2204）は確率的な抽選だけでは出ない年が続くため、
  // 生成分の中に最低1件ずつ現れることを保証する。special 属性まわりの探索経路を
  // 検証したいという用途上、「未観測だから永久に出ない」は避けたい。
  const presentUltras = new Set(
    [...base, ...generated].flatMap((m) => m.parts.filter((p) => isUltra(p.attr_id)).map((p) => p.attr_id)),
  );
  for (const [idStr, cats] of Object.entries(ULTRA_CATEGORIES)) {
    const id = Number(idStr);
    if (presentUltras.has(id)) continue;
    // 対応カテゴリ・極属性なしの生成済み q4 モジュールの通常属性を1つ差し替える
    // （部位数・値・合計はそのままなので、他のルールは崩れない）。
    const victim = generated.find(
      (m) => m.quality === 4 && cats.includes(m.category) && !m.parts.some((p) => isUltra(p.attr_id)),
    );
    if (!victim) {
      console.warn(`極属性 ${id} を差し込める生成モジュールがありません（スキップ）`);
      continue;
    }
    const slot = victim.parts[0];
    slot.attr_id = id;
    slot.attr_name = ULTRA_NAMES[id] ?? `#${id}`;
    presentUltras.add(id);
  }

  const out = [...base, ...generated];
  fs.writeFileSync(outPath, JSON.stringify(out, null, 2), "utf8");
  console.log(`${outPath}: ${base.length} 件 + 生成 ${generated.length} 件 = ${out.length} 件`);
}

main();
