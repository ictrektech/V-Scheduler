//! 推理指标目录（vLLM / SGLang，PromQL，纯函数）
//!
//! 由后端组装、以 `instance="<head IP>:<serve_port>"` 过滤，前端（echarts）按 profile 渲染。
//! 设计见 `specs/docs/design/modelinfer/inference-metrics.md`（SGLang 章节 D-MI-SGH-04）：
//!
//! - vLLM 目录 MUST 兼容 0.9.x 与 0.29.x 的命名差异：
//!   KV-cache `vllm:kv_cache_usage_perc` **or** `vllm:gpu_cache_usage_perc`；输出 token 间隔
//!   `vllm:inter_token_latency_seconds` **or** `vllm:time_per_output_token_seconds`；
//! - SGLang 目录（`sglang:` 前缀）按标签折叠：瞬时 gauge 用 `max`、累计 counter 用
//!   `sum(rate(...))`、直方图分位用 `sum by (le)`；吞吐 / 速率 counter 的 `rate` 窗口随查询步长
//!   自适应（[`crate::monitor::prometheus::rate_window_secs`]）。
//!
//! 不存在的指标由 prometheus 返回空序列（前端空态），MUST NOT 报错。

use crate::inference::InferenceEngine;
use crate::monitor::prometheus::{MetricQuery, esc};

/// 指标分类（前端 profile 过滤键）
pub const CATEGORY: &str = "inference";
/// vLLM prometheus job（= consul 服务名）
pub const SOURCE: &str = crate::inference::VLLM_SERVICE_NAME;
/// SGLang prometheus job（= consul 服务名）
pub const SGLANG_SOURCE: &str = crate::inference::SGLANG_SERVICE_NAME;

/// 按引擎组装指标目录（纯函数）
///
/// - `vllm` → [`vllm_metric_catalog`]；
/// - `sglang` → [`sglang_metric_catalog`]；
/// - 其它引擎（`comfyui` / `unknown`）→ 空目录（前端不展示图表，不报错）。
pub fn catalog_for(engine: &str, instance: &str, rate_window_secs: u64) -> Vec<MetricQuery> {
    match InferenceEngine::parse(engine) {
        InferenceEngine::Vllm => vllm_metric_catalog(instance, rate_window_secs),
        InferenceEngine::Sglang => sglang_metric_catalog(instance, rate_window_secs),
        _ => Vec::new(),
    }
}

/// 生成 vLLM 指标目录（纯函数）
///
/// `rate_window_secs`：吞吐 / 速率类 counter 的 `rate()` 滑动窗口（秒），由查询步长推导
/// （见 `monitor::prometheus::rate_window_secs`）。短窗口对齐 vLLM 日志的瞬时吞吐语义，
/// 避免 5 分钟窗口把突发生成严重稀释；直方图分位与前缀命中率保留 `[5m]`（样本量）。
pub fn vllm_metric_catalog(instance: &str, rate_window_secs: u64) -> Vec<MetricQuery> {
    let i = esc(instance);
    // 吞吐 / 速率类 counter 的 rate 窗口（如 "30s" / "144s" / "300s"）
    let rw = format!("{rate_window_secs}s");
    let mq = |key: &str, name: &str, unit: &str, query: String| MetricQuery {
        key: key.to_string(),
        name: name.to_string(),
        unit: unit.to_string(),
        source: SOURCE.to_string(),
        category: CATEGORY.to_string(),
        query,
    };
    // 直方图分位（`base` 为不含 `_bucket` 的指标名）
    let hq = |q: f64, base: &str, i: &str| {
        format!(
            "histogram_quantile({q}, sum by (le) (rate({base}_bucket{{instance=\"{i}\"}}[5m])))"
        )
    };
    // KV-cache 使用率（跨版本：新名 or 旧名）
    let kv = |i: &str| {
        format!(
            "max(vllm:kv_cache_usage_perc{{instance=\"{i}\"}}) * 100 or max(vllm:gpu_cache_usage_perc{{instance=\"{i}\"}}) * 100"
        )
    };
    // 输出 token 间隔（跨版本）
    let tpot = |q: f64, i: &str| {
        format!(
            "{new} or {old}",
            new = hq(q, "vllm:inter_token_latency_seconds", i),
            old = hq(q, "vllm:time_per_output_token_seconds", i)
        )
    };
    vec![
        // ---- 概览：请求队列 / KV-cache / 吞吐 ----
        mq(
            "vllm_running",
            "运行中请求",
            "",
            format!("max(vllm:num_requests_running{{instance=\"{i}\"}})"),
        ),
        mq(
            "vllm_waiting",
            "排队请求",
            "",
            format!("max(vllm:num_requests_waiting{{instance=\"{i}\"}})"),
        ),
        mq("vllm_kv_cache", "KV-cache 使用率", "%", kv(&i)),
        mq(
            "vllm_gen_throughput",
            "输出吞吐",
            "tok/s",
            format!("sum(rate(vllm:generation_tokens_total{{instance=\"{i}\"}}[{rw}]))"),
        ),
        mq(
            "vllm_prompt_throughput",
            "输入吞吐",
            "tok/s",
            format!("sum(rate(vllm:prompt_tokens_total{{instance=\"{i}\"}}[{rw}]))"),
        ),
        // ---- 时延（分位）----
        mq("vllm_ttft_p50", "首 token 时延（P50）", "s", hq(0.5, "vllm:time_to_first_token_seconds", &i)),
        mq("vllm_ttft_p95", "首 token 时延（P95）", "s", hq(0.95, "vllm:time_to_first_token_seconds", &i)),
        mq("vllm_tpot_p50", "输出 token 间隔（P50）", "s", tpot(0.5, &i)),
        mq("vllm_tpot_p95", "输出 token 间隔（P95）", "s", tpot(0.95, &i)),
        mq("vllm_e2e_p50", "端到端时延（P50）", "s", hq(0.5, "vllm:e2e_request_latency_seconds", &i)),
        mq("vllm_e2e_p95", "端到端时延（P95）", "s", hq(0.95, "vllm:e2e_request_latency_seconds", &i)),
        mq("vllm_e2e_p99", "端到端时延（P99）", "s", hq(0.99, "vllm:e2e_request_latency_seconds", &i)),
        mq("vllm_queue_time", "排队耗时（P95）", "s", hq(0.95, "vllm:request_queue_time_seconds", &i)),
        mq("vllm_prefill_time", "预填充耗时（P95）", "s", hq(0.95, "vllm:request_prefill_time_seconds", &i)),
        mq("vllm_decode_time", "解码耗时（P95）", "s", hq(0.95, "vllm:request_decode_time_seconds", &i)),
        // ---- 缓存 / 抢占 / 成功 ----
        mq(
            "vllm_prefix_hit",
            "前缀缓存命中率",
            "%",
            format!(
                "100 * sum(rate(vllm:prefix_cache_hits_total{{instance=\"{i}\"}}[5m])) / clamp_min(sum(rate(vllm:prefix_cache_queries_total{{instance=\"{i}\"}}[5m])), 1)"
            ),
        ),
        mq(
            "vllm_preemptions",
            "抢占速率",
            "/s",
            format!("sum(rate(vllm:num_preemptions_total{{instance=\"{i}\"}}[{rw}]))"),
        ),
        mq(
            "vllm_success",
            "成功请求速率（按完成原因）",
            "/s",
            format!("sum by (finished_reason) (rate(vllm:request_success_total{{instance=\"{i}\"}}[{rw}]))"),
        ),
    ]
}

/// 生成 SGLang 指标目录（纯函数，`source="modelinfer-sglang"`、`category="inference"`）
///
/// 聚合约定（多 rank / 多进程标签折叠）：
/// - 瞬时 gauge（运行中 / 排队 / token 使用率 / 缓存命中 / 撤回）→ `max(...)`；
/// - 累计 counter（tokens / 中断请求）→ `sum(rate(...))`；
/// - 直方图分位 → `histogram_quantile(q, sum by (le) (rate(..._bucket[5m])))`（固定 5m 保样本量）。
///
/// 跨版本回退：
/// - Token 使用率：`sglang:token_usage * 100` **or** `num_used_tokens / max_total_num_tokens * 100`；
/// - 输出吞吐：`sglang:gen_throughput` **or** `sum(rate(generation_tokens_total[w]))`；
/// - TPOT：`inter_token_latency_seconds` **or** `request_time_per_output_token_seconds`。
///
/// `rate_window_secs` 语义同 [`vllm_metric_catalog`]（由查询步长推导）。
pub fn sglang_metric_catalog(instance: &str, rate_window_secs: u64) -> Vec<MetricQuery> {
    let i = esc(instance);
    let rw = format!("{rate_window_secs}s");
    let mq = |key: &str, name: &str, unit: &str, query: String| MetricQuery {
        key: key.to_string(),
        name: name.to_string(),
        unit: unit.to_string(),
        source: SGLANG_SOURCE.to_string(),
        category: CATEGORY.to_string(),
        query,
    };
    // 直方图分位（`base` 为不含 `_bucket` 的指标名）；合并 model_name / is_streaming 等标签
    let hq = |q: f64, base: &str| {
        format!(
            "histogram_quantile({q}, sum by (le) (rate({base}_bucket{{instance=\"{i}\"}}[5m])))"
        )
    };
    // Token 使用率（占 KV-cache 容量比例 0~1 → %）
    let token_usage = format!(
        "max(sglang:token_usage{{instance=\"{i}\"}}) * 100 \
         or max(sglang:num_used_tokens{{instance=\"{i}\"}}) / clamp_min(max(sglang:max_total_num_tokens{{instance=\"{i}\"}}), 1) * 100"
    );
    // 输出吞吐：优先 SGLang 自算 gauge，缺失时回退累计 counter 的 rate
    let gen_throughput = format!(
        "max(sglang:gen_throughput{{instance=\"{i}\"}}) \
         or sum(rate(sglang:generation_tokens_total{{instance=\"{i}\"}}[{rw}]))"
    );
    // 输出 token 间隔（token 加权 inter_token_latency or 按请求等权 request_time_per_output_token）
    let tpot = |q: f64| {
        format!(
            "{new} or {old}",
            new = hq(q, "sglang:inter_token_latency_seconds"),
            old = hq(q, "sglang:request_time_per_output_token_seconds")
        )
    };
    vec![
        // ---- 概览：请求队列 / token 使用率 / 吞吐 ----
        mq(
            "sglang_running",
            "运行中请求",
            "",
            format!("max(sglang:num_running_reqs{{instance=\"{i}\"}})"),
        ),
        mq(
            "sglang_waiting",
            "排队请求",
            "",
            format!("max(sglang:num_queue_reqs{{instance=\"{i}\"}})"),
        ),
        mq("sglang_token_usage", "Token 使用率（KV-cache）", "%", token_usage),
        mq("sglang_gen_throughput", "输出吞吐", "tok/s", gen_throughput),
        mq(
            "sglang_prompt_throughput",
            "输入吞吐",
            "tok/s",
            format!("sum(rate(sglang:prompt_tokens_total{{instance=\"{i}\"}}[{rw}]))"),
        ),
        // ---- 时延（分位）----
        mq("sglang_ttft_p50", "首 token 时延（P50）", "s", hq(0.5, "sglang:time_to_first_token_seconds")),
        mq("sglang_ttft_p95", "首 token 时延（P95）", "s", hq(0.95, "sglang:time_to_first_token_seconds")),
        mq("sglang_tpot_p50", "输出 token 间隔（P50）", "s", tpot(0.5)),
        mq("sglang_tpot_p95", "输出 token 间隔（P95）", "s", tpot(0.95)),
        mq("sglang_e2e_p50", "端到端时延（P50）", "s", hq(0.5, "sglang:e2e_request_latency_seconds")),
        mq("sglang_e2e_p95", "端到端时延（P95）", "s", hq(0.95, "sglang:e2e_request_latency_seconds")),
        mq("sglang_e2e_p99", "端到端时延（P99）", "s", hq(0.99, "sglang:e2e_request_latency_seconds")),
        mq("sglang_queue_time", "排队耗时（P95）", "s", hq(0.95, "sglang:queue_time_seconds")),
        // ---- 缓存 / 撤回 / 成功 ----
        mq(
            "sglang_cache_hit",
            "缓存命中率",
            "%",
            format!("max(sglang:cache_hit_rate{{instance=\"{i}\"}}) * 100"),
        ),
        mq(
            "sglang_retracted",
            "撤回（retract）请求数",
            "",
            format!("max(sglang:num_retracted_reqs{{instance=\"{i}\"}})"),
        ),
        mq(
            "sglang_aborted",
            "中断请求速率",
            "/s",
            format!("sum(rate(sglang:num_aborted_requests_total{{instance=\"{i}\"}}[{rw}]))"),
        ),
        // 仅统计推理 API 端点（过滤 /health、/metrics 等内部端点噪声），按 HTTP 状态码分组
        mq(
            "sglang_success",
            "请求速率（按状态码）",
            "/s",
            format!(
                "sum by (status_code) (rate(sglang:http_responses_total{{instance=\"{i}\",endpoint=~\"/v1/.*\"}}[{rw}]))"
            ),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keys_source_category_and_instance() {
        let c = vllm_metric_catalog("192.168.8.100:8000", 30);
        let keys: Vec<&str> = c.iter().map(|m| m.key.as_str()).collect();
        for k in [
            "vllm_running",
            "vllm_waiting",
            "vllm_kv_cache",
            "vllm_gen_throughput",
            "vllm_prompt_throughput",
            "vllm_ttft_p50",
            "vllm_ttft_p95",
            "vllm_tpot_p50",
            "vllm_tpot_p95",
            "vllm_e2e_p50",
            "vllm_e2e_p95",
            "vllm_e2e_p99",
            "vllm_queue_time",
            "vllm_prefill_time",
            "vllm_decode_time",
            "vllm_prefix_hit",
            "vllm_preemptions",
            "vllm_success",
        ] {
            assert!(keys.contains(&k), "缺少指标 {k}");
        }
        assert!(c.iter().all(|m| m.source == "modelinfer-vllm" && m.category == "inference"));
        assert!(c.iter().all(|m| m.query.contains("192.168.8.100:8000")));
        // 吞吐 / 速率类用自适应窗口；直方图分位保留 5m
        let bykey = |k: &str| c.iter().find(|m| m.key == k).unwrap().query.clone();
        assert!(bykey("vllm_gen_throughput").contains("[30s]"));
        assert!(bykey("vllm_prompt_throughput").contains("[30s]"));
        assert!(bykey("vllm_success").contains("[30s]"));
        assert!(bykey("vllm_preemptions").contains("[30s]"));
        assert!(bykey("vllm_ttft_p50").contains("[5m]"));
        assert!(bykey("vllm_prefix_hit").contains("[5m]"));
    }

    #[test]
    fn catalog_cross_version_fallbacks() {
        let c = vllm_metric_catalog("10.0.0.1:8000", 30);
        let bykey = |k: &str| c.iter().find(|m| m.key == k).unwrap().query.clone();
        // KV-cache 新旧名兼容
        let kv = bykey("vllm_kv_cache");
        assert!(kv.contains("vllm:kv_cache_usage_perc"));
        assert!(kv.contains("vllm:gpu_cache_usage_perc"));
        assert!(kv.contains(" or "));
        // TPOT 新旧名兼容（inter_token_latency / time_per_output_token）
        let tpot = bykey("vllm_tpot_p95");
        assert!(tpot.contains("vllm:inter_token_latency_seconds_bucket"));
        assert!(tpot.contains("vllm:time_per_output_token_seconds_bucket"));
        assert!(tpot.contains(" or "));
        // TTFT 直方图分位
        assert!(bykey("vllm_ttft_p50").contains("histogram_quantile(0.5"));
        assert!(bykey("vllm_ttft_p50").contains("vllm:time_to_first_token_seconds_bucket"));
        // 成功速率按完成原因分组
        assert!(bykey("vllm_success").contains("by (finished_reason)"));
    }

    #[test]
    fn catalog_rate_window_propagates() {
        let c = vllm_metric_catalog("10.0.0.1:8000", 144);
        let bykey = |k: &str| c.iter().find(|m| m.key == k).unwrap().query.clone();
        assert!(bykey("vllm_gen_throughput").contains("[144s]"));
        assert!(bykey("vllm_success").contains("[144s]"));
        // 直方图不受窗口参数影响
        assert!(bykey("vllm_tpot_p50").contains("[5m]"));
    }

    #[test]
    fn sglang_catalog_keys_source_and_filters() {
        let c = sglang_metric_catalog("10.0.0.1:30000", 30);
        let keys: Vec<&str> = c.iter().map(|m| m.key.as_str()).collect();
        for k in [
            "sglang_running",
            "sglang_waiting",
            "sglang_token_usage",
            "sglang_gen_throughput",
            "sglang_prompt_throughput",
            "sglang_ttft_p50",
            "sglang_ttft_p95",
            "sglang_tpot_p50",
            "sglang_tpot_p95",
            "sglang_e2e_p50",
            "sglang_e2e_p95",
            "sglang_e2e_p99",
            "sglang_queue_time",
            "sglang_cache_hit",
            "sglang_retracted",
            "sglang_aborted",
            "sglang_success",
        ] {
            assert!(keys.contains(&k), "缺少指标 {k}");
        }
        assert!(c.iter().all(|m| m.source == SGLANG_SOURCE && m.category == "inference"));
        assert!(c.iter().all(|m| m.query.contains("10.0.0.1:30000")));
        assert!(!c.iter().any(|m| m.query.contains("vllm:")));
        let bykey = |k: &str| c.iter().find(|m| m.key == k).unwrap().query.clone();
        // gauge 折叠：max
        assert!(bykey("sglang_running").contains("max(sglang:num_running_reqs"));
        assert!(bykey("sglang_waiting").contains("max(sglang:num_queue_reqs"));
        assert!(bykey("sglang_cache_hit").contains("max(sglang:cache_hit_rate"));
        // counter 折叠：sum(rate(...))，窗口随参数
        assert!(bykey("sglang_prompt_throughput").contains("sum(rate(sglang:prompt_tokens_total"));
        assert!(bykey("sglang_aborted").contains("[30s]"));
        // 直方图分位固定 5m + sum by (le)
        assert!(bykey("sglang_ttft_p95").contains("histogram_quantile(0.95"));
        assert!(bykey("sglang_ttft_p95").contains("sum by (le)"));
        assert!(bykey("sglang_ttft_p95").contains("[5m]"));
        // 成功速率：仅 /v1 端点 + 按状态码分组
        assert!(bykey("sglang_success").contains("by (status_code)"));
        assert!(bykey("sglang_success").contains(r#"endpoint=~"/v1/.*""#));
    }

    #[test]
    fn sglang_catalog_cross_version_fallbacks() {
        let c = sglang_metric_catalog("10.0.0.1:30000", 30);
        let bykey = |k: &str| c.iter().find(|m| m.key == k).unwrap().query.clone();
        // token 使用率回退
        let tu = bykey("sglang_token_usage");
        assert!(tu.contains("sglang:token_usage"));
        assert!(tu.contains("sglang:num_used_tokens"));
        assert!(tu.contains("sglang:max_total_num_tokens"));
        assert!(tu.contains(" or "));
        // 输出吞吐回退
        let gt = bykey("sglang_gen_throughput");
        assert!(gt.contains("sglang:gen_throughput"));
        assert!(gt.contains("sglang:generation_tokens_total"));
        assert!(gt.contains(" or "));
        // TPOT 双指标
        let tpot = bykey("sglang_tpot_p95");
        assert!(tpot.contains("sglang:inter_token_latency_seconds_bucket"));
        assert!(tpot.contains("sglang:request_time_per_output_token_seconds_bucket"));
        assert!(tpot.contains(" or "));
    }

    #[test]
    fn catalog_for_dispatches_by_engine() {
        let v = catalog_for("vllm", "10.0.0.1:8000", 30);
        assert!(v.iter().all(|m| m.source == "modelinfer-vllm"));
        let s = catalog_for("sglang", "10.0.0.1:30000", 30);
        assert!(s.iter().all(|m| m.source == "modelinfer-sglang"));
        assert!(catalog_for("comfyui", "10.0.0.1:8188", 30).is_empty());
        assert!(catalog_for("unknown", "10.0.0.1:8000", 30).is_empty());
        assert!(catalog_for("", "10.0.0.1:8000", 30).is_empty());
    }
}