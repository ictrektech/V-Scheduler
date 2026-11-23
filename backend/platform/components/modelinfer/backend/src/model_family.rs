//! 模型家族推断（后端 SSOT）。
//!
//! 契约见 `openspec/changes/modelinfer-list-pagination/design.md` §5：本项目**只在此处**维护
//! 家族关键词登记表与推断算法；前端仅保留 `family → 标签 / 品牌色 / 图标` 的**呈现映射**
//! （`frontend/src/utils/modelFamily.ts`），不再自行推断，避免两份关键词表漂移。
//!
//! 推断语义（与 change 28 前端实现逐条对齐）：
//! - 文本来源优先级：`tags` > `model`（权重 id）> `model_id`（卡片 id）> `name`（展示名）；
//! - 文本若含 `/`（权重 id 形如 `org/weight-name`）按 `/` 分隔为多段，**从后往前**逐段匹配，
//!   命中即返回；未命中再回退到更靠前的段。避免 `nvidia/GLM-5.3-Flash-NVFP4` 中无关的
//!   组织前缀 `nvidia`（→ `nemotron`）覆盖真正的模型家族 `glm`；
//! - 单个文本段内：ASCII 关键词按 token **等值或前缀**匹配；含非 ASCII（中文等）关键词按
//!   **子串**匹配；
//! - 同一段内取**最长命中关键词**（避免 `deepseek-r1-distill-qwen` 被归到 `qwen`）；
//! - 同长度命中按登记表顺序稳定兜底；
//! - 全部未命中返回 [`GENERIC_FAMILY`]。
//!
//! 登记表**只含推断所需信息**（家族标识 / 关键词 / 是否参与推断 / 顺序），不含展示用
//! 标签与品牌色（那些属于前端呈现层）。

/// 兜底家族标识
pub const GENERIC_FAMILY: &str = "generic";

/// 一个家族的推断定义
#[derive(Debug, Clone, Copy)]
pub struct FamilyDef {
    /// 家族标识（同时是前端图标文件名，不含扩展名）
    pub family: &'static str,
    /// 是否参与自动推断（工具类如 vllm / ollama 不参与，避免与模型家族冲突）
    pub inferable: bool,
    /// 匹配关键词（ASCII 词按 token 前缀匹配，含非字母数字 / 中文者按子串匹配）
    pub keywords: &'static [&'static str],
}

/// 家族登记表（顺序即同长度命中时的稳定兜底优先级）
pub const FAMILIES: &[FamilyDef] = &[
    FamilyDef { family: "qwen", inferable: true, keywords: &["qwen", "tongyi", "通义", "千问"] },
    FamilyDef { family: "deepseek", inferable: true, keywords: &["deepseek", "深度求索"] },
    FamilyDef { family: "glm", inferable: true, keywords: &["glm", "chatglm", "zhipu", "智谱", "清言"] },
    FamilyDef { family: "kimi", inferable: true, keywords: &["kimi", "moonshot", "月之暗面"] },
    FamilyDef { family: "minimax", inferable: true, keywords: &["minimax", "abab", "hailuo", "海螺"] },
    FamilyDef { family: "internlm", inferable: true, keywords: &["internlm", "书生", "浦语"] },
    FamilyDef { family: "yi", inferable: true, keywords: &["yi", "01-ai", "lingyiwanwu", "零一万物"] },
    FamilyDef { family: "baichuan", inferable: true, keywords: &["baichuan", "百川"] },
    FamilyDef { family: "hunyuan", inferable: true, keywords: &["hunyuan", "混元"] },
    FamilyDef { family: "stepfun", inferable: true, keywords: &["stepfun", "阶跃"] },
    FamilyDef { family: "doubao", inferable: true, keywords: &["doubao", "豆包"] },
    FamilyDef { family: "ernie", inferable: true, keywords: &["ernie", "文心", "baidu", "百度"] },
    FamilyDef { family: "nemotron", inferable: true, keywords: &["nemotron", "nvidia"] },
    FamilyDef { family: "llama", inferable: true, keywords: &["llama", "meta", "骆驼"] },
    FamilyDef { family: "mistral", inferable: true, keywords: &["mistral", "mixtral", "devstral", "magistral"] },
    FamilyDef { family: "gemma", inferable: true, keywords: &["gemma"] },
    FamilyDef { family: "gemini", inferable: true, keywords: &["gemini"] },
    FamilyDef { family: "phi", inferable: true, keywords: &["phi", "microsoft"] },
    FamilyDef { family: "command", inferable: true, keywords: &["command", "cohere", "aya"] },
    FamilyDef { family: "falcon", inferable: true, keywords: &["falcon", "tii"] },
    FamilyDef { family: "solar", inferable: true, keywords: &["solar", "upstage"] },
    FamilyDef { family: "olmo", inferable: true, keywords: &["olmo", "allenai", "ai2"] },
    FamilyDef { family: "dbrx", inferable: true, keywords: &["dbrx", "databricks"] },
    FamilyDef { family: "arctic", inferable: true, keywords: &["arctic", "snowflake"] },
    FamilyDef { family: "gpt", inferable: true, keywords: &["gpt", "openai"] },
    FamilyDef { family: "claude", inferable: true, keywords: &["claude", "anthropic"] },
    FamilyDef { family: "grok", inferable: true, keywords: &["grok", "xai"] },
    // 工具 / 生态：可供显式展示，但不参与模型家族自动推断
    FamilyDef { family: "huggingface", inferable: false, keywords: &["huggingface", "hf"] },
    FamilyDef { family: "vllm", inferable: false, keywords: &["vllm"] },
    FamilyDef { family: "ollama", inferable: false, keywords: &["ollama"] },
];

/// 全部已知家族标识（含工具类）
pub fn known_families() -> impl Iterator<Item = &'static str> {
    FAMILIES.iter().map(|f| f.family)
}

/// 关键词是否为纯 ASCII 字母数字（决定按 token 前缀匹配 vs 子串匹配）
fn is_ascii_keyword(keyword: &str) -> bool {
    !keyword.is_empty()
        && keyword.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// 单条命中
struct Hit {
    family: &'static str,
    length: usize,
}

/// 在单个文本段中匹配家族（见模块注释的匹配规则）
fn match_segment(text: &str) -> Option<Hit> {
    let raw = text.to_lowercase();
    // token 切分：仅保留 ASCII 字母数字与 CJK 统一表意文字（与前端 `[^a-z0-9\u4e00-\u9fff]+` 对齐）
    let tokens: Vec<&str> = raw
        .split(|c: char| !(c.is_ascii_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&c)))
        .filter(|t| !t.is_empty())
        .collect();

    let mut best: Option<Hit> = None;
    for def in FAMILIES {
        if !def.inferable {
            continue;
        }
        for keyword in def.keywords {
            let k = keyword.to_lowercase();
            let hit = if is_ascii_keyword(&k) {
                tokens.iter().any(|t| *t == k || t.starts_with(&k))
            } else {
                raw.contains(&k)
            };
            if hit && best.as_ref().is_none_or(|b| k.chars().count() > b.length) {
                best = Some(Hit {
                    family: def.family,
                    length: k.chars().count(),
                });
            }
        }
    }
    best
}

/// 在单个文本中匹配家族：按 `/` 分隔为多段，**从后往前**逐段匹配，返回首个命中。
///
/// 模型权重 id 形如 `org/weight-name`，其中 `org` 可能是与权重实际家族无关的组织前缀
/// （如 `nvidia/GLM-5.3-Flash-NVFP4` 的 `nvidia`）：若整串一起匹配，前缀中的关键词可能
/// 因更长而覆盖真正的模型家族。故优先匹配靠后的权重段，命中即返回；未命中再回退到前段。
fn match_text(text: &str) -> Option<Hit> {
    text.split('/').rev().find_map(match_segment)
}

/// 推断模型家族标识。
///
/// 优先级 `tags` > 权重 id > 卡片 id > 展示名；全部未命中返回 [`GENERIC_FAMILY`]。
/// 纯函数，MUST NOT 抛错。
pub fn infer(
    tags: &[String],
    model: Option<&str>,
    model_id: Option<&str>,
    name: Option<&str>,
) -> &'static str {
    let tags_joined = tags.join(" ");
    let sources: [&str; 4] = [
        tags_joined.as_str(),
        model.unwrap_or(""),
        model_id.unwrap_or(""),
        name.unwrap_or(""),
    ];
    for text in sources {
        if text.trim().is_empty() {
            continue;
        }
        if let Some(hit) = match_text(text) {
            return hit.family;
        }
    }
    GENERIC_FAMILY
}

/// 家族标识是否已知（未知时前端回落通用样式，后端可用于校验）
pub fn is_known(family: &str) -> bool {
    FAMILIES.iter().any(|f| f.family == family)
}

/// 便捷入口：从 `serde_json` 形态的多语言展示名中取纯文本后推断。
pub fn infer_from_json_name(
    tags: &[String],
    model: Option<&str>,
    model_id: Option<&str>,
    name: Option<&serde_json::Value>,
) -> &'static str {
    let name_text = name.and_then(display_text);
    infer(tags, model, model_id, name_text.as_deref())
}

/// 从 `serde_json` 多语言值取展示文本（对象取 `zh-CN` → `en` → 首个字符串值）
pub fn display_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        serde_json::Value::Object(map) => {
            for key in ["zh-CN", "zh", "en"] {
                if let Some(serde_json::Value::String(s)) = map.get(key)
                    && !s.trim().is_empty()
                {
                    return Some(s.clone());
                }
            }
            map.values()
                .find_map(|v| v.as_str().filter(|s| !s.trim().is_empty()).map(str::to_string))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn infers_from_weight_id() {
        assert_eq!(infer(&[], Some("deepseek-ai/DeepSeek-V4.1-Flash"), None, None), "deepseek");
        assert_eq!(infer(&[], Some("Qwen/Qwen3.8-27B"), None, None), "qwen");
        assert_eq!(infer(&[], Some("ZhipuAI/GLM-5.3-Flash"), None, None), "glm");
    }

    #[test]
    fn tags_take_priority() {
        assert_eq!(
            infer(&tags(&["qwen"]), Some("deepseek-ai/DeepSeek-V3"), None, None),
            "qwen"
        );
    }

    #[test]
    fn longest_keyword_wins() {
        assert_eq!(
            infer(&[], Some("deepseek-ai/DeepSeek-R1-Distill-Qwen-14B"), None, None),
            "deepseek"
        );
    }

    #[test]
    fn weight_id_matches_last_path_segment_first() {
        // 组织前缀 `nvidia`（→ nemotron）不得覆盖权重段真正家族 `glm`
        assert_eq!(infer(&[], Some("nvidia/GLM-5.3-Flash-NVFP4"), None, None), "glm");
        assert_eq!(infer(&[], Some("RedHatAI/GLM-5.3-Flash-NVFP4"), None, None), "glm");
        // 权重段自身即家族，靠后优先
        assert_eq!(infer(&[], Some("nvidia/Nemotron-4"), None, None), "nemotron");
        assert_eq!(infer(&[], Some("meta-llama/Llama-3.1-8B"), None, None), "llama");
    }

    #[test]
    fn weight_id_falls_back_to_earlier_segment() {
        // 权重段无家族 → 回退到组织段
        assert_eq!(infer(&[], Some("deepseek-ai/mystery-model"), None, None), "deepseek");
        // 权重段仅命中不可推断的工具关键词 → 继续回退到组织段
        assert_eq!(infer(&[], Some("nvidia/vllm"), None, None), "nemotron");
        // 多段路径：从后往前找到首个命中
        assert_eq!(infer(&[], Some("org/deepseek-ai/GLM-5.3"), None, None), "glm");
        // 任意段均无命中 → generic
        assert_eq!(infer(&[], Some("foo/bar/baz"), None, None), GENERIC_FAMILY);
    }

    #[test]
    fn falls_back_to_lower_priority_sources() {
        assert_eq!(infer(&[], None, Some("com.ictrek.glm-5.3-flash-gb10"), None), "glm");
        assert_eq!(infer(&[], None, None, Some("Kimi K2 大模型")), "kimi");
    }

    #[test]
    fn matches_chinese_keywords_by_substring() {
        assert_eq!(infer(&[], None, None, Some("通义千问 2.5 7B")), "qwen");
        assert_eq!(infer(&[], None, None, Some("智谱清言")), "glm");
    }

    #[test]
    fn ascii_keyword_prefix_does_not_over_match() {
        // `yi` 不得前缀命中 `easy`
        assert_eq!(infer(&[], Some("foo/easy-model"), None, None), GENERIC_FAMILY);
        // 但可前缀命中 `yi-34b`
        assert_eq!(infer(&[], Some("01-ai/Yi-34B"), None, None), "yi");
    }

    #[test]
    fn unknown_returns_generic_without_panic() {
        assert_eq!(infer(&[], Some("some-unknown-org/mystery-model"), None, None), GENERIC_FAMILY);
        assert_eq!(infer(&[], None, None, None), GENERIC_FAMILY);
        assert_eq!(infer(&[], None, None, Some("")), GENERIC_FAMILY);
    }

    #[test]
    fn infers_from_json_multilingual_name() {
        let name = serde_json::json!({ "zh-CN": "DeepSeek-V4.1-Flash（DGX Spark ×4）", "en": "DeepSeek" });
        assert_eq!(infer_from_json_name(&[], None, None, Some(&name)), "deepseek");

        let plain = serde_json::json!("Qwen3.5-4B（H100 / vLLM FP8）");
        assert_eq!(
            infer_from_json_name(&tags(&["vllm"]), Some("Qwen/Qwen3.5-4B"), None, Some(&plain)),
            "qwen"
        );
    }

    #[test]
    fn tool_families_do_not_participate() {
        assert_eq!(infer(&tags(&["vllm"]), None, None, None), GENERIC_FAMILY);
        assert!(is_known("vllm"));
        assert!(!is_known("nope"));
    }
}