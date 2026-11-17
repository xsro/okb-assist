//! 重复文献检测（标题相似 / DOI 重复）与检测记录落盘。
//!
//! 相比早期的「首字符分桶 + 与组代表比编辑距离」，这里改了四件事：
//!
//! 1. **候选生成改为分块（blocking）**：按内容词（拉丁词 / 中文二元组）建低频词倒排索引，
//!    外加归一化标题精确键。首词不同、写法略有出入的同一个标题不再漏掉；
//!    每篇的候选数有上限，比较次数随库规模线性增长而不是平方。
//! 2. **判定用双证据**：字符编辑距离相似度 + 内容词重合度（Jaccard / 包含度）；
//!    短标题单独收紧门槛，避免「Battery Modeling」这类词少撞车。
//! 3. **聚类改为并查集**：相似关系按传递合并，组分取组内最弱的一条边，
//!    不再出现「先来者当代表、后来者只跟代表比」造成的漏聚。
//! 4. **两种依据互相印证**：标题组带 DOI 证据（`same_doi` / `doi_conflict`），
//!    DOI 组带标题一致性（`title_score` / `title_conflict`）——
//!    同 DOI 但标题完全不像，更可能是 DOI 填错；标题很像但 DOI 不同，是 preprint 与正式版的常见情形。
//!
//! 一篇文档的 `i18n.en.title` 作为**别名**一起参与标题比较，
//! 所以「中文题名条目」与「英文题名条目」（Crossref 回填的）能互认。
//!
//! 结果记录写到 `system.json` 的 `tmp_folder_path`（默认 `data/tmp/duplicate-scan.json`），
//! **只存分组与成员 ID**：成员元数据在读取时按 ID 回查，页面因此永远显示最新值，
//! 被删除或合并掉的条目会自然从历史分组里消失（不足 2 篇的组不再输出）。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::Settings;
use crate::services::pdf_meta;

/// 记录文件格式版本（字段变更时用于兼容处理）
pub const RECORD_VERSION: u32 = 1;
/// 记录文件名（位于 `system.json` 的 `tmp_folder_path` 下）
pub const RECORD_FILE: &str = "duplicate-scan.json";

// ── 判定门槛 ─────────────────────────────────────────────
/// 长标题直接采信所需的编辑距离相似度
const EDIT_ACCEPT: f64 = 0.90;
/// 宽松门槛：编辑距离一般，但内容词重合度必须够高
const EDIT_LOOSE: f64 = 0.78;
const JACCARD_ACCEPT: f64 = 0.65;
/// 内容词几乎完全重合（词序/标点/单复数写法差异）
const JACCARD_STRONG: f64 = 0.85;
/// 副标题/系列名式差异：短标题的内容词几乎全包含在长标题里
const CONTAINMENT_ACCEPT: f64 = 0.90;
const CONTAINMENT_MIN_TOKENS: usize = 4;
/// 短标题（归一化后不足 15 字符，或内容词少于 3 个）近乎要求逐字一致
const SHORT_MAX_LEN: usize = 15;
const SHORT_MIN_TOKENS: usize = 3;
const SHORT_EDIT_ACCEPT: f64 = 0.95;
/// 只靠内容词重合（词同、写法差很多）命中时仍要求的编辑相似度下限。
/// 它同时决定这类对的分带宽度，不设下限就得做整表 DP。
const EDIT_FLOOR_STRONG_JAC: f64 = 0.60;

// ── 分块参数（控制比较次数）─────────────────────────────
/// 每篇取几个「最有区分度」的内容词作为候选键
const MAX_BLOCK_TOKENS: usize = 8;
/// 出现在超过该比例的文档里的词太泛，不做候选键（下限见 `df_limit` 的 `max(4, ..)`）
const MAX_DF_RATIO: f64 = 0.15;
/// 单篇主动比较次数上限
const MAX_PAIRS_PER_DOC: usize = 256;
/// DOI 组内标题一致性检查的成员数上限（组过大时不再做平方级比较）
const MAX_TITLE_CHECK_MEMBERS: usize = 40;

/// 英文虚词：不参与内容词重合度计算（对中文按二元组切分，虚词单独见 `CJK_STOPCHARS`）
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "and", "or", "for", "on", "in", "to", "with", "from", "at", "by",
    "as", "is", "are", "be", "been", "into", "onto", "than", "then", "that", "this", "these",
    "those", "its", "their", "via", "vs",
];
/// 中文连接/结构虚字：所在二元组直接丢弃（两个标题同样处理，不影响可比性）
const CJK_STOPCHARS: &[char] = &['的', '了', '与', '和', '及', '或', '之', '而'];

/// 参与检测的文档（只取检测需要的列，不拉整行 `Document`）
#[derive(Debug, Clone)]
pub struct ScanDoc {
    pub id: i64,
    /// 基础题名（PDF 主要语言）
    pub title: String,
    /// `i18n.en.title`：参与比较的别名，可为空
    pub title_alt: String,
    pub doi: String,
}

/// 一个标题的比较键：归一化串 + 字符数组 + 内容词集合。
///
/// `chars` 预先算好：候选对数量与文档数成正比，逐对再 `chars().collect()` 会把
/// 分配开销摊到百万次比较上。
#[derive(Debug, Clone)]
struct Key {
    norm: String,
    chars: Vec<char>,
    tokens: HashSet<String>,
}

/// 标题相似组（持久化只存成员 ID，元数据读取时回查）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleGroup {
    /// 代表标题：组内最小 ID 那篇的归一化主标题，用于展示，同时是前端的分组 key
    pub normalized_title: String,
    /// 组内**最弱**一条边的相似度（并查集是单链聚类，取最保守的值）
    pub score: f64,
    pub doc_ids: Vec<i64>,
    /// 组内至少两篇 DOI 相同（标题与 DOI 双重命中）
    #[serde(default)]
    pub same_doi: bool,
    /// 组内出现两个以上不同 DOI（preprint / 正式版常见，不等于不是重复）
    #[serde(default)]
    pub doi_conflict: bool,
    /// 有成员是靠「主标题 ↔ 英文题名别名」互认的（跨语言命中）
    #[serde(default)]
    pub cross_lang: bool,
}

/// DOI 重复组
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoiGroup {
    /// 归一化 DOI（见 `pdf_meta::doi_key`），前端的分组 key
    pub doi_key: String,
    /// 组内第一个非空原始 DOI，仅用于展示
    pub doi: String,
    pub doc_ids: Vec<i64>,
    /// 组内标题一致性：每篇都能与组内某篇对上的最低分数；`None` = 成员过多未检查
    pub title_score: Option<f64>,
    /// 有成员与组内任何其他篇的标题都对不上（更可能是 DOI 填错，而非同一篇文献）
    #[serde(default)]
    pub title_conflict: bool,
}

/// 一次检测的完整记录（落盘内容）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRecord {
    #[serde(default)]
    pub version: u32,
    /// 本次检测完成时间（UTC，秒级）
    #[serde(default)]
    pub generated_at: String,
    /// 库签名（见 [`signature`]）：与当前库不一致说明结果已过期
    #[serde(default)]
    pub signature: String,
    /// 参与检测的文档数
    #[serde(default)]
    pub scanned_docs: usize,
    #[serde(default)]
    pub title: Vec<TitleGroup>,
    #[serde(default)]
    pub doi: Vec<DoiGroup>,
}

/// 库签名：`文档数|MAX(updated_at)`。
///
/// 上传、改元数据、合并都会让它变化，因此不需要额外维护「脏标记」。
pub fn signature(doc_count: i64, max_updated_at: &str) -> String {
    format!("{}|{}", doc_count, max_updated_at.trim())
}

/// 跑一次完整检测（两种依据），返回待落盘的记录
pub fn scan(docs: &[ScanDoc], signature: String) -> ScanRecord {
    ScanRecord {
        version: RECORD_VERSION,
        generated_at: crate::utils::now_iso(),
        signature,
        scanned_docs: docs.len(),
        title: title_groups(docs),
        doi: doi_groups(docs),
    }
}

// ── 标题算法 ─────────────────────────────────────────────

/// 标题归一化：全角折半角 → 小写 → 非字母数字压成单个空格。
///
/// 中日韩字符属于 alphanumeric，会原样保留（`tokenize` 再按二元组切分）。
pub fn normalize_title_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_space = true;
    for c in raw.to_lowercase().chars() {
        let c = match c {
            // 全角字母/数字/标点（中文文献封面常见）折成半角
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            c => c,
        };
        if c.is_alphanumeric() {
            out.push(c);
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF
    )
}

fn is_cjk_stop(c: char) -> bool {
    CJK_STOPCHARS.contains(&c)
}

/// 切出内容词集合：拉丁词按空格切（虚词丢弃、单字母丢弃），
/// CJK 连续段切成二元组（单字段落保留单字）。
fn tokenize(norm: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    for word in norm.split(' ') {
        if word.is_empty() {
            continue;
        }
        let chars: Vec<char> = word.chars().collect();
        if chars.iter().any(|c| is_cjk(*c)) {
            if chars.len() == 1 {
                if !is_cjk_stop(chars[0]) {
                    set.insert(word.to_string());
                }
            } else {
                for pair in chars.windows(2) {
                    if is_cjk_stop(pair[0]) || is_cjk_stop(pair[1]) {
                        continue;
                    }
                    set.insert(pair.iter().collect());
                }
            }
        } else if chars.len() > 1 && !STOPWORDS.contains(&word) {
            set.insert(word.to_string());
        }
    }
    set
}

/// 一篇文档的全部比较键：主标题 + 英文题名别名（重复的不再加一遍）
fn keys_of(doc: &ScanDoc) -> Vec<Key> {
    let mut out: Vec<Key> = Vec::with_capacity(2);
    for raw in [&doc.title, &doc.title_alt] {
        let norm = normalize_title_key(raw);
        if norm.is_empty() || out.iter().any(|k| k.norm == norm) {
            continue;
        }
        let tokens = tokenize(&norm);
        let chars: Vec<char> = norm.chars().collect();
        out.push(Key { norm, chars, tokens });
    }
    out
}

/// 有界（分带）Levenshtein：只关心距离是否 `<= max_dist`。
///
/// 逐行只在反对角带内计算，并跟踪该行最小值；一旦整行都超过 `max_dist` 就提前放弃
/// （`None`）。绝大多数候选对（分块只共享一两个词）在前几行就被排除，
/// 这是把比较成本压到与文档数成正比的关键——否则 7000 篇 × 数百候选对 × 逐字 DP
/// 会让一次检测跑上几分钟。
fn levenshtein_band(a: &[char], b: &[char], max_dist: usize) -> Option<usize> {
    let (a, b) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    if a.len() - b.len() > max_dist {
        return None;
    }
    let n = b.len();
    // 一侧为空串时带内没有单元可算，距离就是另一侧长度（上面的长度差检查已保证它够小）
    if n == 0 {
        return Some(a.len());
    }
    // 带外单元置为不可达（max_dist + 1），行内滚动数组
    let mut prev = vec![usize::MAX; n + 1];
    let mut curr = vec![usize::MAX; n + 1];
    for j in 0..=n.min(max_dist) {
        prev[j] = j;
    }
    for i in 1..=a.len() {
        let j_lo = i.saturating_sub(max_dist).max(1);
        let j_hi = (i + max_dist).min(n);
        curr.fill(usize::MAX);
        curr[0] = if i <= max_dist { i } else { usize::MAX };
        let mut row_min = usize::MAX;
        for j in j_lo..=j_hi {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let sub = prev[j - 1].saturating_add(cost);
            let del = prev[j].saturating_add(1);
            let ins = curr[j - 1].saturating_add(1);
            let v = sub.min(del).min(ins);
            curr[j] = v;
            if v < row_min {
                row_min = v;
            }
        }
        // 整行都超出允许的编辑距离：后面只会更大，不必再算
        if row_min > max_dist {
            return None;
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    let d = prev[n];
    if d > max_dist {
        None
    } else {
        Some(d)
    }
}

/// 内容词重合度：(交集, 并集, 较小集合大小)
fn token_stats(a: &HashSet<String>, b: &HashSet<String>) -> (usize, usize, usize) {
    let inter = a.iter().filter(|t| b.contains(*t)).count();
    let union = a.len() + b.len() - inter;
    (inter, union, a.len().min(b.len()))
}

/// 综合分数（只用于展示，判定走上层的门槛）：编辑距离为主、内容词重合为辅
fn blend(edit: f64, jac: f64) -> f64 {
    (0.6 * edit + 0.4 * jac).min(1.0).max(0.0)
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// 两个标题的相似度；`None` = 判定为不相似。
///
/// 接受的组合：近乎逐字一致 / 内容词几乎重合（词序与标点差异）/
/// 编辑距离一般但内容词高度重合 / 一方只是多了副标题。
///
/// 顺序刻意是「先便宜的重合度、后贵的编辑距离」：先由内容词判定这一对**最少**要
/// 达到多少编辑相似度（`min_sim`），再用它换算分带宽度 `max_dist`——
/// 长度差本身就够排除一大批候选，剩下的也能在带内早早放弃。
fn pair_score(a: &Key, b: &Key) -> Option<f64> {
    let (a_len, b_len) = (a.chars.len(), b.chars.len());
    if a_len == 0 || b_len == 0 {
        return None;
    }
    if a.norm == b.norm {
        return Some(1.0);
    }
    let max_len = a_len.max(b_len);
    let (inter, union, smaller) = token_stats(&a.tokens, &b.tokens);
    let jac = if union == 0 { 0.0 } else { inter as f64 / union as f64 };
    let cont = if smaller == 0 { 0.0 } else { inter as f64 / smaller as f64 };
    // 内容词已经几乎重合：这类对可能只靠重合度就被接受，编辑相似度允许更低
    let strong_jac = jac >= JACCARD_STRONG;
    let loose = strong_jac
        || jac >= JACCARD_ACCEPT
        || (cont >= CONTAINMENT_ACCEPT && smaller >= CONTAINMENT_MIN_TOKENS);
    let min_sim = if strong_jac {
        EDIT_FLOOR_STRONG_JAC
    } else if loose {
        EDIT_LOOSE
    } else {
        EDIT_ACCEPT
    };
    let max_dist = ((max_len as f64) * (1.0 - min_sim)).ceil() as usize;
    // 编辑距离 >= 长度差：先把明显不可能排掉（分带内部也会做同样的检查）
    if a_len.abs_diff(b_len) > max_dist {
        return None;
    }
    let dist = levenshtein_band(&a.chars, &b.chars, max_dist)?;
    let edit = 1.0 - (dist as f64 / max_len as f64);

    // 短标题信息量太少，只认近乎完全一致的写法
    if max_len < SHORT_MAX_LEN || smaller < SHORT_MIN_TOKENS {
        return if edit >= SHORT_EDIT_ACCEPT {
            Some(blend(edit, jac))
        } else {
            None
        };
    }
    if edit >= EDIT_ACCEPT || jac >= JACCARD_STRONG {
        return Some(blend(edit, jac));
    }
    if edit >= EDIT_LOOSE
        && (jac >= JACCARD_ACCEPT
            || (cont >= CONTAINMENT_ACCEPT && smaller >= CONTAINMENT_MIN_TOKENS))
    {
        return Some(blend(edit, jac));
    }
    None
}

/// 两篇文档之间最优的一组标题比较
///
/// 返回 `(分数, 是否靠别名互认)`；`cross` 为 true 表示这次命中至少用到了一侧的英文题名别名。
fn best_pair(a: &[Key], b: &[Key]) -> Option<(f64, bool)> {
    let mut best: Option<(f64, bool)> = None;
    for (ai, ka) in a.iter().enumerate() {
        for (bi, kb) in b.iter().enumerate() {
            if let Some(s) = pair_score(ka, kb) {
                if best.map_or(true, |(bs, _)| s > bs) {
                    best = Some((s, ai != 0 || bi != 0));
                }
            }
        }
    }
    best
}

/// 并查集（路径压缩 + 按秩合并）
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            let root = self.find(self.parent[x]);
            self.parent[x] = root;
        }
        self.parent[x]
    }

    fn union(&mut self, a: usize, b: usize) {
        let (mut a, mut b) = (self.find(a), self.find(b));
        if a == b {
            return;
        }
        if self.rank[a] < self.rank[b] {
            std::mem::swap(&mut a, &mut b);
        }
        self.parent[b] = a;
        if self.rank[a] == self.rank[b] {
            self.rank[a] += 1;
        }
    }
}

/// 标题相似聚类：归一化精确键 + 低频内容词分块 → 逐对判定 → 并查集合并
pub fn title_groups(docs: &[ScanDoc]) -> Vec<TitleGroup> {
    let n = docs.len();
    if n < 2 {
        return Vec::new();
    }
    let keys: Vec<Vec<Key>> = docs.iter().map(keys_of).collect();

    // 1. 每篇的内容词全集（含别名）与文档频率 df
    let mut doc_tokens: Vec<HashSet<String>> = Vec::with_capacity(n);
    let mut df: HashMap<String, usize> = HashMap::new();
    for ks in &keys {
        let mut set: HashSet<String> = HashSet::new();
        for k in ks {
            for t in &k.tokens {
                if set.insert(t.clone()) {
                    *df.entry(t.clone()).or_insert(0) += 1;
                }
            }
        }
        doc_tokens.push(set);
    }

    // 2. 倒排索引：一个词只连「有区分度」的文档 —— 太泛的词（df 高于比例）连不进候选，
    //    只出现在一篇里的词又产生不了候选对，两者都不作为分块键。
    let df_limit = (((n as f64) * MAX_DF_RATIO).ceil() as usize).max(4);
    let mut inverted: HashMap<String, Vec<usize>> = HashMap::new();
    let mut blocks: Vec<Vec<String>> = Vec::with_capacity(n);
    for (i, set) in doc_tokens.iter().enumerate() {
        let mut picked: Vec<String> = set
            .iter()
            .filter(|t| {
                df.get(t.as_str())
                    .map_or(false, |c| *c >= 2 && *c <= df_limit)
            })
            .map(|t| t.to_string())
            .collect();
        // 每篇只保留最有区分度的若干个词，避免高频词把候选炸开
        picked.sort_unstable_by(|a, b| df[a].cmp(&df[b]).then_with(|| a.cmp(b)));
        picked.truncate(MAX_BLOCK_TOKENS);
        for t in &picked {
            inverted.entry(t.clone()).or_default().push(i);
        }
        blocks.push(picked);
    }

    let mut uf = UnionFind::new(n);
    let mut edges: Vec<(usize, usize, f64, bool)> = Vec::new();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();

    // 3. 归一化标题完全相同：直接建边（分数 1.0），不依赖分块是否选中同一个词
    let mut by_norm: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for (i, ks) in keys.iter().enumerate() {
        for (ai, k) in ks.iter().enumerate() {
            if !k.norm.is_empty() {
                by_norm.entry(k.norm.clone()).or_default().push((i, ai));
            }
        }
    }
    for members in by_norm.values() {
        for (x, &(i, ai)) in members.iter().enumerate() {
            for &(j, aj) in &members[x + 1..] {
                // 同一篇的主标题与英文题名可能本来就相同（英文文献），不构成对
                if i == j {
                    continue;
                }
                let (a, b) = if i < j { (i, j) } else { (j, i) };
                if seen.insert((a, b)) {
                    uf.union(a, b);
                    edges.push((a, b, 1.0, ai != 0 || aj != 0));
                }
            }
        }
    }

    // 4. 分块候选逐对判定
    let mut scored = vec![0usize; n];
    for i in 0..n {
        for tk in &blocks[i] {
            if scored[i] >= MAX_PAIRS_PER_DOC {
                break;
            }
            let members = match inverted.get(tk) {
                Some(m) => m,
                None => continue,
            };
            for &j in members {
                if j == i {
                    continue;
                }
                if scored[i] >= MAX_PAIRS_PER_DOC {
                    break;
                }
                let (a, b) = if i < j { (i, j) } else { (j, i) };
                if !seen.insert((a, b)) {
                    continue;
                }
                scored[i] += 1;
                if let Some((s, cross)) = best_pair(&keys[a], &keys[b]) {
                    uf.union(a, b);
                    edges.push((a, b, s, cross));
                }
            }
        }
    }

    // 5. 汇总分组：组分取组内最弱的一条边，DOI 证据按成员实际取值统计
    let mut root = vec![0usize; n];
    for i in 0..n {
        root[i] = uf.find(i);
    }
    let mut members_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        members_of.entry(root[i]).or_default().push(i);
    }
    let mut weakest: HashMap<usize, f64> = HashMap::new();
    let mut cross: HashMap<usize, bool> = HashMap::new();
    for &(a, _, s, x) in &edges {
        let r = root[a];
        weakest.entry(r).and_modify(|v| *v = v.min(s)).or_insert(s);
        let flag = cross.entry(r).or_insert(false);
        *flag = *flag || x;
    }

    let mut out: Vec<TitleGroup> = Vec::new();
    for group in members_of.values() {
        if group.len() < 2 {
            continue;
        }
        let mut sorted = group.clone();
        sorted.sort_by_key(|&i| docs[i].id);
        let r = root[sorted[0]];
        // 代表标题：最小 ID 那篇的主标题，主标题缺失时退到它的英文题名
        let rep = keys[sorted[0]]
            .iter()
            .map(|k| k.norm.clone())
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        let doi_keys: Vec<String> = sorted
            .iter()
            .filter_map(|&i| pdf_meta::doi_key(&docs[i].doi))
            .collect();
        let distinct = doi_keys.iter().collect::<HashSet<&String>>().len();
        out.push(TitleGroup {
            normalized_title: rep,
            score: round3(weakest.get(&r).copied().unwrap_or(1.0)),
            doc_ids: sorted.iter().map(|&i| docs[i].id).collect(),
            same_doi: doi_keys.len() >= 2 && distinct == 1,
            doi_conflict: distinct >= 2,
            cross_lang: cross.get(&r).copied().unwrap_or(false),
        });
    }
    out.sort_by(|a, b| a.doc_ids.cmp(&b.doc_ids));
    out
}

// ── DOI 算法 ─────────────────────────────────────────────

/// DOI 重复分组：`pdf_meta::doi_key` 归一化后精确分组，并回填组内标题一致性。
pub fn doi_groups(docs: &[ScanDoc]) -> Vec<DoiGroup> {
    let mut map: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, d) in docs.iter().enumerate() {
        if let Some(key) = pdf_meta::doi_key(&d.doi) {
            map.entry(key).or_default().push(i);
        }
    }

    let mut out: Vec<DoiGroup> = Vec::new();
    for (key, mut members) in map {
        if members.len() < 2 {
            continue;
        }
        members.sort_by_key(|&i| docs[i].id);
        let display_doi = members
            .iter()
            .find_map(|&i| (!docs[i].doi.trim().is_empty()).then(|| docs[i].doi.trim().to_string()))
            .unwrap_or_else(|| key.clone());
        let (title_score, title_conflict) = title_agreement(&members, docs);
        out.push(DoiGroup {
            doi_key: key,
            doi: display_doi,
            doc_ids: members.iter().map(|&i| docs[i].id).collect(),
            title_score,
            title_conflict,
        });
    }
    out.sort_by(|a, b| a.doc_ids.cmp(&b.doc_ids));
    out
}

/// 组内标题一致性：每篇与组内其他篇的最佳分数，取其中的最低值。
///
/// - `title_conflict = true`：有成员跟组内谁都对不上（同 DOI 却不像同一篇文献，多为 DOI 填错）；
/// - 成员数超过 `MAX_TITLE_CHECK_MEMBERS` 时返回 `None`，不做平方级比较。
fn title_agreement(members: &[usize], docs: &[ScanDoc]) -> (Option<f64>, bool) {
    if members.len() > MAX_TITLE_CHECK_MEMBERS {
        return (None, false);
    }
    let keys: Vec<Vec<Key>> = members.iter().map(|&i| keys_of(&docs[i])).collect();
    let mut bests: Vec<Option<f64>> = Vec::with_capacity(keys.len());
    for a in 0..keys.len() {
        let mut best: Option<f64> = None;
        for b in 0..keys.len() {
            if a == b {
                continue;
            }
            if let Some((s, _)) = best_pair(&keys[a], &keys[b]) {
                best = Some(match best {
                    Some(v) => v.max(s),
                    None => s,
                });
            }
        }
        bests.push(best);
    }
    let conflict = bests.iter().any(|b| b.is_none());
    let matched: Vec<f64> = bests.iter().flatten().copied().collect();
    let score = if matched.is_empty() {
        0.0
    } else {
        matched.into_iter().fold(f64::INFINITY, f64::min)
    };
    (Some(round3(score)), conflict)
}

// ── 记录读写（tmp_folder_path）────────────────────────────

/// 检测记录路径：`{tmp_folder_path}/duplicate-scan.json`
pub fn record_path(settings: &Settings) -> PathBuf {
    settings.tmp_folder().join(RECORD_FILE)
}

/// 写入检测记录（原子写：先 `*.tmp` 再 rename），返回落盘路径
pub fn save(settings: &Settings, record: &ScanRecord) -> std::io::Result<PathBuf> {
    let path = record_path(settings);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_string_pretty(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// 读取上次的检测记录；文件不存在返回 `Ok(None)`，内容解析失败返回 `Err`
/// （调用方按「没有记录」处理并打日志，避免坏文件把页面卡死）
pub fn load(settings: &Settings) -> std::io::Result<Option<ScanRecord>> {
    let path = record_path(settings);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<ScanRecord>(&text)
            .map(Some)
            .map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("检测记录 {} 解析失败: {}", path.display(), e),
                )
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: i64, title: &str, alt: &str, doi: &str) -> ScanDoc {
        ScanDoc {
            id,
            title: title.to_string(),
            title_alt: alt.to_string(),
            doi: doi.to_string(),
        }
    }

    fn key(raw: &str) -> Key {
        let norm = normalize_title_key(raw);
        let tokens = tokenize(&norm);
        let chars: Vec<char> = norm.chars().collect();
        Key { norm, chars, tokens }
    }

    fn score_of(a: &str, b: &str) -> Option<f64> {
        pair_score(&key(a), &key(b))
    }

    #[test]
    fn normalize_folds_fullwidth_and_punctuation() {
        assert_eq!(
            normalize_title_key("ＡＢＣ： 深度 学习, v2.0"),
            "abc 深度 学习 v2 0"
        );
        assert_eq!(normalize_title_key("  太空   难   "), "太空 难");
    }

    #[test]
    fn tokenize_splits_cjk_into_bigrams_and_drops_stopwords() {
        let t = tokenize(&normalize_title_key("A survey of deep learning"));
        let want: HashSet<String> = ["survey", "deep", "learning"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(t, want);
        let c = tokenize(&normalize_title_key("深度学习的图像识别"));
        // 「的」参与的二元组被丢弃，其余按相邻二字切分
        assert!(c.contains("深度") && c.contains("学习") && c.contains("图像") && c.contains("识别"));
        assert!(!c.iter().any(|s| s.contains('的')));
    }

    #[test]
    fn accepts_wording_variants_and_subtitle() {
        // 词序/标点/大小写差异
        assert!(score_of("Deep Learning for Image Segmentation", "deep learning, for: image segmentation").is_some());
        // 首词不同（旧的首字符分桶必漏）
        assert!(score_of("A survey of neural networks", "Second survey of neural networks").is_some());
        // 只多了副标题
        assert!(score_of(
            "Deep reinforcement learning for robotics",
            "Deep reinforcement learning for robotics: a review"
        )
        .is_some());
    }

    #[test]
    fn rejects_different_works_and_short_near_collisions() {
        assert!(score_of("Machine translation of text", "Machine translation of speech").is_none());
        assert!(score_of("Battery Modeling", "Battery Management").is_none());
        assert!(score_of("Solar power", "Solar wind").is_none());
    }

    #[test]
    fn clusters_transitively_and_reports_doi_evidence() {
        let docs = vec![
            doc(1, "Deep reinforcement learning for robotics", "", "10.1000/a"),
            doc(2, "Deep reinforcement learning for robotics: a review", "", "10.1000/b"),
            doc(3, "Deep reinforcement learning for roboics", "", ""),
            doc(4, "Unrelated title about hydrology", "", "10.1000/c"),
        ];
        let groups = title_groups(&docs);
        assert_eq!(groups.len(), 1, "三篇应聚成一组");
        assert_eq!(groups[0].doc_ids, vec![1, 2, 3]);
        // 2 与 3 之间靠 1 传递，组分取最弱一条边
        assert!(groups[0].score > 0.7 && groups[0].score < 1.0);
        assert!(groups[0].doi_conflict, "组内出现两个不同 DOI");
        assert!(!groups[0].same_doi);
    }

    #[test]
    fn same_doi_marks_double_hit() {
        let docs = vec![
            doc(5, "城市内涝风险评估", "", "https://doi.org/10.1000/XYZ"),
            doc(6, "城市内涝风险评估", "", "doi:10.1000/xyz"),
        ];
        let groups = title_groups(&docs);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].same_doi);
        assert!(!groups[0].doi_conflict);
        assert_eq!(groups[0].score, 1.0);
    }

    #[test]
    fn english_alias_matches_english_base_title() {
        // 1 是中文条目但带 i18n 英文题名，2 是 Crossref 回填的英文条目：靠别名互认
        let docs = vec![
            doc(
                1,
                "深度强化学习综述",
                "A survey of deep reinforcement learning",
                "",
            ),
            doc(2, "A survey of deep reinforcement learning", "", ""),
        ];
        let groups = title_groups(&docs);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].cross_lang);
        assert_eq!(groups[0].score, 1.0);
    }

    #[test]
    fn doi_groups_flag_title_conflict() {
        let docs = vec![
            doc(1, "深度学习在遥感中的应用", "", "10.1000/same"),
            doc(2, "Deep learning in remote sensing", "", " 10.1000/SAME. "),
            doc(3, "完全无关的标题：混凝土耐久性", "", "10.1000/same"),
        ];
        let groups = doi_groups(&docs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].doc_ids, vec![1, 2, 3]);
        assert!(groups[0].title_conflict, "第 3 篇标题与组内谁都对不上");
        assert!(groups[0].title_score.unwrap_or(1.0) < 0.5);
    }

    #[test]
    fn doi_group_reports_title_agreement() {
        let docs = vec![
            doc(1, "Deep learning for remote sensing", "", "10.1000/x"),
            doc(2, "Deep Learning for Remote Sensing!", "", "10.1000/x"),
        ];
        let groups = doi_groups(&docs);
        assert_eq!(groups[0].doi, "10.1000/x", "展示用的是第一个非空原值");
        assert!(!groups[0].title_conflict);
        assert!(groups[0].title_score.unwrap_or(0.0) >= 0.9);
    }

    #[test]
    fn empty_and_single_doc_inputs_yield_no_groups() {
        assert!(title_groups(&[]).is_empty());
        assert!(doi_groups(&[]).is_empty());
        let one = vec![doc(1, "Only one", "", "10.1/a")];
        assert!(title_groups(&one).is_empty());
        assert!(doi_groups(&one).is_empty());
    }

    #[test]
    fn signature_changes_with_count_and_time() {
        assert_eq!(signature(3, " 2026-01-01 "), "3|2026-01-01");
        assert_ne!(signature(3, "2026-01-01"), signature(4, "2026-01-01"));
    }

    /// 检测记录经由 `tmp_folder_path` 落盘后能原样读回（前端「自动加载上次结果」依赖这一点）
    #[test]
    fn record_roundtrip_through_tmp_folder() {
        use crate::config_manager::ConfigManager;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp").to_string_lossy().replace('\\', "/");
        std::fs::write(dir.path().join("config.json"), "{}").unwrap();
        std::fs::write(
            dir.path().join("system.json"),
            serde_json::json!({ "tmp_folder_path": tmp }).to_string(),
        )
        .unwrap();
        let settings = Settings::new(Arc::new(ConfigManager::for_tests(
            dir.path().join("config.json"),
            dir.path().join("system.json"),
        )));

        let record = ScanRecord {
            version: RECORD_VERSION,
            generated_at: "2026-01-01T00:00:00Z".to_string(),
            signature: signature(7, "2026-01-01 10:00:00"),
            scanned_docs: 7,
            title: vec![TitleGroup {
                normalized_title: "deep learning".to_string(),
                score: 0.912,
                doc_ids: vec![2, 5],
                same_doi: false,
                doi_conflict: true,
                cross_lang: false,
            }],
            doi: Vec::new(),
        };
        let path = save(&settings, &record).unwrap();
        assert!(path.exists());

        let loaded = load(&settings).unwrap().expect("应能读回上次记录");
        assert_eq!(loaded.signature, record.signature);
        assert_eq!(loaded.scanned_docs, 7);
        assert_eq!(loaded.title[0].doc_ids, vec![2, 5]);
        assert!(loaded.title[0].doi_conflict);

        std::fs::remove_file(&path).unwrap();
        // 文件不存在不是错误：前端把这种情况当作「还没有检测记录」
        assert!(load(&settings).unwrap().is_none());
    }

    /// 完整 DP 参照实现，只用于验证 [`levenshtein_band`]
    fn levenshtein_full(a: &[char], b: &[char]) -> usize {
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        let mut curr = vec![0usize; b.len() + 1];
        for i in 1..=a.len() {
            curr[0] = i;
            for j in 1..=b.len() {
                let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
                curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            }
            std::mem::swap(&mut prev, &mut curr);
        }
        prev[b.len()]
    }

    #[test]
    fn banded_levenshtein_matches_full_dp() {
        let cases: Vec<(String, String)> = vec![
            ("kitten 沙发".into(), "sitting 沙发".into()),
            ("深度学习与强化学习综述".into(), "深度强化学习".into()),
            ("".into(), "abc".into()),
            ("a".repeat(30), "b".repeat(28)),
            (
                "consensus algorithms for double integrator dynamics".into(),
                "consensus algorithm of double integrator dynamic".into(),
            ),
            ("完全无关的两条标题甲".into(), "another unrelated title".into()),
        ];
        for (x, y) in cases {
            let a: Vec<char> = x.chars().collect();
            let b: Vec<char> = y.chars().collect();
            let full = levenshtein_full(&a, &b);
            // 带宽刚好等于真实距离时必须给出同一个值
            assert_eq!(levenshtein_band(&a, &b, full), Some(full), "{x} | {y}");
            // 比真实距离窄就应当放弃，而不是给出错误值
            if full > 0 {
                assert_eq!(levenshtein_band(&a, &b, full - 1), None, "{x} | {y}");
            }
            assert_eq!(levenshtein_band(&a, &b, full + 3), Some(full));
        }
    }

    #[test]
    fn long_titles_with_small_edits_still_accept() {
        // 长标题只差几个字：分带宽度按 1 - EDIT_LOOSE 算，绝不能把这类对误排除
        let base = "deep reinforcement learning for multi robot cooperative navigation in unknown environments";
        let variant = "deep reinforcement learning for multi robot cooperative navigation in unknown environment";
        let s = score_of(base, variant).expect("只差一个词尾的长标题应当被接受");
        assert!(s >= 0.9, "分数不应被低估：{s}");
    }

    /// 真实库样本的耗时哨兵（默认跳过）。
    ///
    /// `OKB_DUP_FIXTURE=<docs.json> cargo test --release dup_scan -- --ignored`；
    /// docs.json 是 `[{id,title,alt,doi}]` 数组，从库里导出即可。
    #[test]
    #[ignore]
    fn bench_scan_on_real_fixture() {
        let path = std::env::var("OKB_DUP_FIXTURE").expect("需要 OKB_DUP_FIXTURE 指向 docs.json");
        let text = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        let docs: Vec<ScanDoc> = rows
            .iter()
            .map(|r| ScanDoc {
                id: r["id"].as_i64().unwrap_or(0),
                title: r["title"].as_str().unwrap_or("").to_string(),
                title_alt: r["alt"].as_str().unwrap_or("").to_string(),
                doi: r["doi"].as_str().unwrap_or("").to_string(),
            })
            .collect();
        let started = std::time::Instant::now();
        let record = scan(&docs, "bench".to_string());
        eprintln!(
            "docs={} title_groups={} doi_groups={} 用时={:?}",
            docs.len(),
            record.title.len(),
            record.doi.len(),
            started.elapsed()
        );
    }
}
