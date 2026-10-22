import {apiError} from '../http-client.js';
import { createGatewayI18n } from './locale.js';
import { playgroundStreamEvent } from './playground-stream.js';
export function mountGateway(root, options) {
const i18n = createGatewayI18n(root, options.locale);
const VRouterI18n = i18n;
const VRouterOIDC = { accessToken: options.token, clear: options.clearToken };
i18n.apply();
// V-Router management UI. Vanilla JS, no build step.
"use strict";

// Same-origin absolute paths routed by the VOS Traefik gateway:
//   /app/com.ictrek.v-router/  -> this page
//   /api/com.ictrek.v-router/  -> gateway backend
const API = options.apiBase;

const $ = (id) => root.getElementById(id);
const esc = (s) =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

// The backend supplies the configured host port; the browser knows the VOS host.
const lanBaseURL = (bases) => (bases.lan || "http://<vos-host>:22888/v1")
  .replace("<vos-host>", window.location.hostname).replace("<host-port>", "22888");

const COPY_SVG =
  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>';

const TAB_TITLES = { overview: "总览", models: "模型", upstreams: "上游", playground: "试聊", apidoc: "API 文档" };
const PG_ENDPOINTS = [
  { id: "chat", path: "/chat/completions", label: "Chat Completions", stream: true },
  { id: "responses", path: "/responses", label: "Responses", stream: true },
  { id: "messages", path: "/messages", label: "Anthropic Messages", stream: true },
  { id: "completions", path: "/completions", label: "Text Completions", stream: true },
  { id: "embeddings", path: "/embeddings", label: "Embeddings", stream: false },
  { id: "videos", path: "/videos/generations", label: "Video Generations", stream: false },
];

// Vendor defaults mirror the provider catalog used by ZCode. Base URLs point
// at each vendor's OpenAI-compatible API where one exists; MiMo only exposes an
// Anthropic-compatible endpoint.
const PROVIDER_PRESETS = [
  { id: "openrouter", name: "OpenRouter", base: "https://openrouter.ai/api/v1", protocol: "openai", model: "openai/gpt-5.6-sol" },
  { id: "deepseek", name: "DeepSeek", base: "https://api.deepseek.com/v1", protocol: "openai", model: "deepseek-v4-pro" },
  { id: "bailian", name: "Alibaba Cloud Bailian (China)", base: "https://dashscope.aliyuncs.com/compatible-mode/v1", protocol: "openai", model: "qwen3.8-max" },
  { id: "bailian-intl", name: "Alibaba Cloud Bailian (Intl)", base: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1", protocol: "openai", model: "qwen3.8-max" },
  { id: "zai", name: "Z.ai / GLM", base: "https://api.z.ai/api/paas/v4", protocol: "openai", model: "glm-5.3" },
  { id: "bigmodel", name: "BigModel / GLM", base: "https://open.bigmodel.cn/api/paas/v4", protocol: "openai", model: "glm-5.3" },
  { id: "moonshot", name: "Moonshot Kimi", base: "https://api.moonshot.cn/v1", protocol: "openai", model: "kimi-k3" },
  { id: "minimax", name: "MiniMax", base: "https://api.minimaxi.com/v1", protocol: "openai", model: "MiniMax-M3" },
  { id: "mimo", name: "Xiaomi MiMo", base: "https://api.xiaomimimo.com/anthropic/v1", protocol: "anthropic", model: "mimo-v2.6-pro" },
  { id: "openai", name: "OpenAI", base: "https://api.openai.com/v1", protocol: "openai", model: "gpt-5.6-sol" },
  { id: "anthropic", name: "Anthropic", base: "https://api.anthropic.com/v1", protocol: "anthropic", model: "claude-sonnet-5" },
  { id: "xai", name: "xAI", base: "https://api.x.ai/v1", protocol: "openai", model: "grok-4.6" },
  { id: "opencode-zen", name: "OpenCode Zen", base: "https://opencode.ai/zen/v1", protocol: "openai", model: "glm-5.2" },
  { id: "opencode-go", name: "OpenCode Go", base: "https://opencode.ai/zen/go/v1", protocol: "openai", model: "glm-5.3" },
];

const app = {
  element: id => root.getElementById(id),
  state: { status: null, models: [], upstreams: [], usage: null, usagePeriod: "day", usageQuery: "", usageSort: "total_tokens", tab: "overview", timer: null },

  // ---------- api ----------
  gatewayKey() {
    return localStorage.getItem("v-router-key") || "";
  },
  authHeaders: async function () {
    const headers = {};
    const token = await VRouterOIDC.accessToken();
    if (token) {
      headers["Authorization"] = "Bearer " + token;
    } else if (this.gatewayKey()) {
      headers["Authorization"] = "Bearer " + this.gatewayKey();
    }
    return headers;
  },
  async api(method, path, body, raw = false, retried = false) {
    const headers = await this.authHeaders();
    if (body !== undefined) headers["Content-Type"] = "application/json";
    const res = await fetch(API + path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    if (res.status === 401 && !retried && headers.Authorization && headers.Authorization !== "Bearer " + this.gatewayKey()) {
      VRouterOIDC.clear(headers.Authorization.slice(7));
      return this.api(method, path, body, raw, true);
    }
    if (raw) return res;
    const text = await res.text();
    let data = null;
    try { data = text ? JSON.parse(text) : null; } catch {
      if (res.ok) {
        const sourceMessage = "网关返回了无效的 JSON 响应，请检查服务和访问路径。";
        const err = new Error(VRouterI18n.t(sourceMessage));
        err.sourceMessage = sourceMessage;
        throw err;
      }
      data = null;
    }
    if (!res.ok) {
      const err = apiError(data,res.status);
      throw err;
    }
    return data;
  },

  showError(html, retryFn) {
    const el = $("errorBanner");
    this.errorRetry = retryFn;
    VRouterI18n.bind(el, "innerHTML", () => {
      const message = typeof html === "function" ? html() : html;
      return message + (retryFn ? `<button class="btn" onclick="app.errorRetry()">${VRouterI18n.t("重试登录")}</button>` : "");
    });
    el.hidden = !(typeof html === "function" ? html() : html);
  },
  toast(msg) {
    const el = $("toast");
    el.textContent = msg;
    el.classList.add("show");
    setTimeout(() => el.classList.remove("show"), 2000);
  },
  async copyText(text) {
    try {
      await navigator.clipboard.writeText(text);
      this.toast(VRouterI18n.t("已复制"));
    } catch {
      this.toast(VRouterI18n.t("复制失败，请手动选择复制"));
    }
  },

  // ---------- lifecycle ----------
  async start() {
    this.renderProviderPresets($("ufPreset"));
    this.renderProviderPresets($("regPreset"));
    $("apiKeyInput").addEventListener("change", (e) => {
      localStorage.setItem("v-router-key", e.target.value.trim());
      this.refresh();
    });
    root.querySelectorAll("#nav button").forEach((btn) => {
      btn.addEventListener("click", () => this.switchTab(btn.dataset.tab));
    });
    $("pgModel").addEventListener("change", () => this.renderPlaygroundEndpoints());
    $("pgEndpoint").addEventListener("change", () => {
      this.renderPlaygroundUpstreams();
      this.updatePlaygroundMode();
    });
    $("pgInput").addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing && e.keyCode !== 229) {
        e.preventDefault();
        if (!e.repeat && !this.pgRequest) this.pgSend();
      }
    });
    await this.refresh();
    this.state.timer = setInterval(() => {
      if (options.visible() && !document.hidden && this.state.tab !== "playground" && this.state.tab !== "apidoc") this.refresh();
    }, 15000);
  },

  // 401 diagnostics: distinguish "userinfo unreachable" (config) from
  // "no/invalid token" (login) and guide the fix.
  async diagnoseAuthFailure() {
    let probe = null;
    try {
      probe = await this.api("GET", "/api/v1/auth-probe");
    } catch {}
    const inVOS = window.parent !== window;
    if (probe && !probe.oidc_reachable) {
      this.showError(
        () => `<div>${VRouterI18n.t("登录服务不可达")}：${VRouterI18n.t("网关无法连接")} VOS ${VRouterI18n.t("用户信息服务")}（<span class="mono">${esc(probe.oidc_error || "unknown error")}</span>）。` +
          `${VRouterI18n.t("请尝试重新安装本应用")}（${VRouterI18n.t("使")} OIDC ${VRouterI18n.t("客户端重新注册")}），${VRouterI18n.t("或检查")} VOS_OIDC_USERINFO_URL / VOS_HOST_GW_IP ${VRouterI18n.t("配置")}。</div>`,
      );
      return;
    }
    if (!inVOS) {
      this.showError(
        () => VRouterI18n.t("<div>当前为外部访问模式：管理接口需要 VOS 用户身份或网关 API Key。请填写右上角 API Key，或通过 VOS 侧边栏打开本页面。</div>"),
      );
      $("apiKeyInput").hidden = false;
      return;
    }
    this.showError(() => VRouterI18n.t("<div>已连接 VOS，但未获取到有效登录态，可重试静默登录。</div>"), async () => {
      VRouterOIDC.clear();
      $("errorBanner").hidden = true;
      await this.refresh();
    });
  },

  async refresh() {
    try {
      const [status, models, upstreams, usage] = await Promise.all([
        this.api("GET", "/api/v1/status"),
        this.api("GET", "/api/v1/models"),
        this.api("GET", "/api/v1/upstreams"),
        this.api("GET", "/api/v1/usage?period=" + encodeURIComponent(this.state.usagePeriod)),
      ]);
      const admin = status.is_admin === true;
      this.state = {
        ...this.state,
        status,
        models: models.models,
        upstreams: upstreams.upstreams,
        usage,
        budgets: admin ? this.state.budgets : [],
      };
      this.showError("");
      const catalog = status.catalog_readiness;
      const notice = $("catalogNotice");
      VRouterI18n.bind(notice, "textContent", () => {
        if (!catalog || catalog.ready) return "";
        const pending = Object.values(catalog.backends || {}).includes("initializing");
        return VRouterI18n.t(pending
          ? "正在发现内置模型，列表将自动更新；已就绪模型仍可调用。"
          : "部分内置推理后端不可用，请在模型仓库检查推理服务；其他就绪模型仍可调用。");
      });
      notice.hidden = !notice.textContent;
      this.renderAll();
      if (this.state.tab === "apidoc") this.renderApiDoc();
      if (admin) this.loadBudgets();
    } catch (err) {
      if (err.status === 401) {
        this.renderUser(null);
        await this.diagnoseAuthFailure();
        return;
      }
      this.showError(() => VRouterI18n.t("加载失败：") + esc(VRouterI18n.t(err.sourceMessage || err.message)));
    }
  },

  // Budgets live behind the controller and can be much slower than the gateway
  // itself; load them after the first paint instead of blocking the page.
  async loadBudgets() {
    const seq = this.budgetSeq = (this.budgetSeq || 0) + 1;
    let items = [];
    try {
      const r = await this.api("GET", "/scheduler/api/v1/model-budgets");
      items = r?.data?.items || [];
    } catch {}
    if (seq !== this.budgetSeq) return;
    this.state.budgets = items;
    this.renderModels();
  },

  switchTab(tab) {
    this.state.tab = tab;
    if (tab === 'playground') options.select(tab);
    VRouterI18n.bind($("pageTitle"), "textContent", () => VRouterI18n.t(TAB_TITLES[tab] || tab));
    root.querySelectorAll("#nav button").forEach((b) => b.classList.toggle("active", b.dataset.tab === tab));
    root.querySelectorAll(".tab").forEach((el) => (el.hidden = el.id !== "tab-" + tab));
    if (tab === "playground") this.renderPlaygroundOptions();
    if (tab === "apidoc") this.renderApiDoc();
  },

  changeUsagePeriod(period) {
    this.state.usagePeriod = period;
    this.refresh();
  },

  changeUsageQuery(query) {
    this.state.usageQuery = query.trim().toLowerCase();
    this.renderOverview();
  },

  changeUsageSort(sort) {
    this.state.usageSort = sort;
    this.renderOverview();
  },

  // ---------- rendering ----------
  renderAll(preservePlayground = false) {
    this.renderUser(this.state.status);
    this.renderHero();
    this.renderOverview();
    this.renderModels();
    this.renderUpstreams();
    if (this.state.tab === "playground" && !preservePlayground) this.renderPlaygroundOptions();
    if (this.state.status && this.state.status.key_required) $("apiKeyInput").hidden = false;
  },

  renderUser(status) {
    const chip = $("userChip"), offline = $("userOffline");
    const user = status?.user;
    $("pgForceRow").hidden = options.administrator?.() !== true;
    if (options.administrator?.() !== true) $("pgForceAdmission").checked = false;
    if (user) {
      chip.hidden = false; offline.hidden = true;
      $("userName").textContent = user;
      $("userAvatar").textContent = (user[0] || "?");
      VRouterI18n.bind($("userRole"), "textContent", () => status.is_admin ? VRouterI18n.t("管理员 · 网关 Key") : VRouterI18n.t("VOS 用户"));
    } else {
      chip.hidden = true; offline.hidden = false;
    }
    VRouterI18n.bind($("brandVersion"), "textContent", () => status?.version ? "v" + status.version : VRouterI18n.t("版本不可用"));
  },

  renderHero() {
    const st = this.state.status;
    if (!st) return;
    const bases = st.base_urls || {};
    const healthy = st.upstreams_healthy > 0;
    VRouterI18n.bind($("overviewHero"), "innerHTML", () => `
      <div class="eyebrow">ONE ENDPOINT. EVERY MODEL.</div><h2><span class="dot ${healthy ? "ok" : "bad"}"></span>${healthy ? VRouterI18n.t("网关运行中") : VRouterI18n.t("暂无健康上游")}</h2>
      <div class="hero-sub">${st.upstreams_healthy}/${st.upstreams_total} ${VRouterI18n.t("个上游健康")} · ${st.models} ${VRouterI18n.t("个模型可用")} · ${
        st.user ? `${VRouterI18n.t("已登录")} ${esc(st.user)}` : VRouterI18n.t("内网免鉴权模式")
      }</div>
      <div class="hero-endpoints">
        ${chipWithCopy(VRouterI18n.t("VOS 内"), bases.internal)}
        ${chipWithCopy(VRouterI18n.t("局域网调用"), lanBaseURL(bases))}
        ${chipWithCopy(VRouterI18n.t("同域接入"), bases.same_origin || API + "/v1")}
      </div>`);
    root.querySelectorAll("#overviewHero .copy").forEach((btn, i) => {
      btn.onclick = () => this.copyText([bases.internal, lanBaseURL(bases), bases.same_origin || API + "/v1"][i]);
    });
  },

  renderOverview() {
    const st = this.state.status;
    if (!st) return;
    const fmt = (iso) => (iso ? iso.replace("T", " ").slice(5, 16) : "");
    VRouterI18n.bind($("overviewStats"), "innerHTML", () => [
      stat(iconServer(), `${st.upstreams_healthy}<span class="unit">/${st.upstreams_total}</span>`, VRouterI18n.t("健康上游")),
      stat(iconLayers(), st.models, VRouterI18n.t("可用模型")),
      stat(iconActivity(), st.requests_total, `${VRouterI18n.t("累计请求")}<div class="hint" style="margin-top:2px">${VRouterI18n.t("自")} ${esc(fmt(st.usage_since))}</div>`),
      stat(iconTimer(), (this.state.usage?.totals?.latency_ms_avg ?? 0) + " ms", VRouterI18n.t("平均延迟")),
    ].join(""));

    const usage = this.state.usage;
    const activePeriod = usage?.period || this.state.usagePeriod;
    root.querySelectorAll(".usage-period button").forEach((button) => {
      const active = button.dataset.period === activePeriod;
      button.classList.toggle("active", active);
      button.setAttribute("aria-pressed", String(active));
    });
    $("usageSort").value = this.state.usageSort;
    const totals = usage?.totals || {};
    const token = (v, n) => v?.usage_requests ? Number(v[n] || 0).toLocaleString() : "—";
    const coverage = totals.requests ? `${totals.usage_requests || 0}/${totals.requests} ${VRouterI18n.t("个请求上报")} Token` : VRouterI18n.t("暂无请求");
    VRouterI18n.bind($("usageCoverage"), "textContent", () => `${coverage}${usage?.since ? ` · ${VRouterI18n.t("自")} ${fmt(usage.since)}` : ""}`);
    VRouterI18n.bind($("usageSummary"), "innerHTML", () => [
      [VRouterI18n.t("输入 Token"), token(totals, "input_tokens"), VRouterI18n.t("发送给模型")],
      [VRouterI18n.t("输出 Token"), token(totals, "output_tokens"), VRouterI18n.t("模型生成")],
      [VRouterI18n.t("总 Token"), token(totals, "total_tokens"), VRouterI18n.t("输入与输出合计")],
      [VRouterI18n.t("请求数"), Number(totals.requests || 0).toLocaleString(), totals.errors ? `${totals.errors} ${VRouterI18n.t("个失败")}` : VRouterI18n.t("全部正常")],
    ].map(([label, value, hint]) => `<div class="usage-kpi"><div class="usage-kpi-label">${label}</div><div class="usage-kpi-value">${value}</div><div class="usage-kpi-hint">${hint}</div></div>`).join(""));
    const matches = ([key, value]) => !this.state.usageQuery || `${key} ${value.name || ""}`.toLowerCase().includes(this.state.usageQuery);
    const sorted = (entries) => entries.filter(matches).sort((a, b) => {
      if (this.state.usageSort === "name") return a[0].localeCompare(b[0]);
      return Number(b[1][this.state.usageSort] || 0) - Number(a[1][this.state.usageSort] || 0) || a[0].localeCompare(b[0]);
    });
    const per = sorted(Object.entries(usage?.per_model || {}));
    VRouterI18n.bind($("usageSince"), "textContent", () => usage?.since ? VRouterI18n.t("（自 ") + fmt(usage.since) + "）" : "");
    VRouterI18n.bind($("usagePerModel"), "innerHTML", () => per.length
      ? table(
          [VRouterI18n.t("模型"), VRouterI18n.t("请求"), VRouterI18n.t("输入"), VRouterI18n.t("输出"), VRouterI18n.t("合计"), VRouterI18n.t("Token 上报"), VRouterI18n.t("失败"), VRouterI18n.t("平均延迟")],
          per.map(([k, v]) => [
            `<span class="mono">${esc(k)}</span>`,
            v.requests,
            token(v, "input_tokens"),
            token(v, "output_tokens"),
            token(v, "total_tokens"),
            `${v.usage_requests || 0}/${v.requests}`,
            v.errors ? `<span class="pill bad">${v.errors}</span>` : "0",
            v.latency_ms_avg + " ms",
          ]),
          [false, true, true, true, true, true, true, true],
        )
      : `<div class="empty">${this.state.usageQuery ? VRouterI18n.t("没有符合筛选条件的模型") : VRouterI18n.t("暂无请求记录")}</div>`);

    const perUpstream = sorted(Object.entries(usage?.per_upstream || {}));
    VRouterI18n.bind($("usagePerUpstream"), "innerHTML", () => perUpstream.length
      ? table(
          [VRouterI18n.t("上游"), VRouterI18n.t("请求"), VRouterI18n.t("输入"), VRouterI18n.t("输出"), VRouterI18n.t("合计"), VRouterI18n.t("Token 上报"), VRouterI18n.t("失败")],
          perUpstream.map(([k, v]) => [
            `${esc(v.name || k)}<div class="hint mono">${esc(k)}</div>`,
            v.requests,
            token(v, "input_tokens"),
            token(v, "output_tokens"),
            token(v, "total_tokens"),
            `${v.usage_requests || 0}/${v.requests}`,
            v.errors ? `<span class="pill bad">${v.errors}</span>` : "0",
          ]),
          [false, true, true, true, true, true, true],
        )
      : `<div class="empty">${this.state.usageQuery ? VRouterI18n.t("没有符合筛选条件的上游") : VRouterI18n.t("暂无上游请求记录")}</div>`);

    const perUser = Object.entries(usage?.per_user || {});
    VRouterI18n.bind($("usagePerUser"), "innerHTML", () => perUser.length
      ? table(
          [VRouterI18n.t("用户"), VRouterI18n.t("请求"), VRouterI18n.t("失败"), VRouterI18n.t("平均延迟")],
          perUser.map(([k, v]) => [
            `<span class="pill kind">${esc(k)}</span>`,
            v.requests,
            v.errors ? `<span class="pill bad">${v.errors}</span>` : "0",
            v.latency_ms_avg + " ms",
          ]),
          [false, true, true, true],
        )
      : `<div class="empty">${VRouterI18n.t("暂无用户请求记录")}</div>`);

    const recent = usage?.recent || [];
    VRouterI18n.bind($("usageRecent"), "innerHTML", () => recent.length
      ? table(
          [VRouterI18n.t("时间"), VRouterI18n.t("用户"), VRouterI18n.t("模型"), VRouterI18n.t("上游"), VRouterI18n.t("接口"), VRouterI18n.t("输入"), VRouterI18n.t("输出"), VRouterI18n.t("状态"), VRouterI18n.t("延迟"), VRouterI18n.t("流式")],
          recent.slice(0, 30).map((r) => [
            `<span class="mono">${esc(fmt(r.time))}</span>`,
            `<span class="pill kind">${esc(r.user)}</span>`,
            `<span class="mono">${esc(r.model)}</span>`,
            esc(r.upstream_name),
            esc(r.endpoint),
            r.usage_reported ? Number(r.input_tokens || 0).toLocaleString() : "—",
            r.usage_reported ? Number(r.output_tokens || 0).toLocaleString() : "—",
            r.status < 400 ? `<span class="pill ok">${r.status}</span>` : `<span class="pill bad">${r.status}</span>`,
            r.latency_ms + " ms",
            r.stream ? VRouterI18n.t("是") : VRouterI18n.t("否"),
          ]),
          [false, false, false, false, false, true, true, false, true, false],
        )
      : `<div class="empty">${VRouterI18n.t("暂无请求记录")}</div>`);
  },

  renderModels() {
    const models = this.state.models;
    const admin = this.state.status?.is_admin === true;
    VRouterI18n.bind($("modelsTable"), "innerHTML", () => models.length
      ? table(
          [VRouterI18n.t("模型 ID"), VRouterI18n.t("类型"), VRouterI18n.t("来源"), VRouterI18n.t("上游"), VRouterI18n.t("资源预算"), VRouterI18n.t("操作")],
          models.map((m) => {
            const budget = this.budgetFor(m);
            const actions = [];
            if (!m.short && m.kind !== "service") {
              actions.push(`<button class="btn" onclick="app.tryModel('${esc(m.qualified)}')">${VRouterI18n.t("测试")}</button>`);
            }
            if (m.upstream_source === "api" && m.upstream_manageable !== false) {
              actions.push(`<button class="btn" onclick="app.openModelDialog('${esc(m.qualified)}', '${esc(m.upstream_id)}')">${VRouterI18n.t("编辑元数据")}</button>`);
            }
            if (admin && !m.upstream_direct && m.kind !== "service") {
              if (m.direct) {
                actions.push(`<button class="btn" onclick="app.setModelDirect('${esc(m.upstream_id)}', '${esc(m.id)}', false)">${VRouterI18n.t("取消放行")}</button>`);
              } else if (budget && budget.mode === "direct") {
                actions.push(`<button class="btn" onclick="app.restoreBudget('${esc(m.qualified)}', '${esc(m.upstream_id)}')">${VRouterI18n.t("恢复默认")}</button>`);
              } else {
                actions.push(`<button class="btn" onclick="app.setModelDirect('${esc(m.upstream_id)}', '${esc(m.id)}', true)">${VRouterI18n.t("直接放行")}</button>`);
                actions.push(`<button class="btn" onclick="app.openBudgetDialog('${esc(m.qualified)}', '${esc(m.upstream_id)}')">${VRouterI18n.t("校准预算")}</button>`);
                if (budget && budget.mode !== "automatic") {
                  actions.push(`<button class="btn" onclick="app.restoreBudget('${esc(m.qualified)}', '${esc(m.upstream_id)}')">${VRouterI18n.t("恢复默认")}</button>`);
                }
              }
            }
            return [
              `<span class="mono">${esc(m.short ? m.id : m.qualified)}</span>${m.short ? ` <span class="pill kind">${VRouterI18n.t("短名")}</span>` : ""}`,
              ({embedding:"Embedding",chat:VRouterI18n.t("对话"),unknown:VRouterI18n.t("由上游决定"),service:VRouterI18n.t("服务接口")})[m.kind] || esc(m.kind),
              `<span class="pill ${m.upstream_kind}">${kindLabel(m.upstream_kind)}</span>`,
              esc(m.upstream_name),
              this.budgetCell(m, budget),
              actions.length ? `<div class="actions">${actions.join("")}</div>` : "",
            ];
          }),
        )
      : `<div class="empty">${this.state.status?.catalog_readiness?.ready === false ? VRouterI18n.t("模型目录尚未就绪。") : VRouterI18n.t("暂无模型") + "。" + VRouterI18n.t("请在模型仓库下载模型，或在上游页添加后端。")}</div>`);
  },

  budgetFor(model) {
    const budgets = this.state.budgets || [];
    const bound = budgets.find((budget) => budget.model_id === model.qualified && !budget.upstream_url);
    if (bound) return bound;
    const url = (model.upstream_url || "").replace(/\/+$/, "");
    if (!url) return null;
    return budgets.find((budget) => budget.upstream_url
      && budget.upstream_url.replace(/\/+$/, "") === url
      && budget.model_id === model.id) || null;
  },

  budgetCell(model, budget) {
    if (model.upstream_direct) return `<span class="pill warn">${VRouterI18n.t("直接放行（上游）")}</span>`;
    if (model.direct) return `<span class="pill warn">${VRouterI18n.t("直接放行（模型）")}</span>`;
    if (model.kind === "service") return `<span class="muted">${VRouterI18n.t("服务直通")}</span>`;
    if (!budget) return `<span class="muted">${VRouterI18n.t("自动（默认）")}</span>`;
    if (budget.mode === "direct") return `<span class="pill warn">${VRouterI18n.t("直接放行")}</span>`;
    if (budget.mode === "automatic") return `<span class="muted">${VRouterI18n.t("自动（默认）")}</span>`;
    const r = budget.resources || {};
    const parts = [
      `${Number(r.cpu_millicores || 0)} mCPU`,
      `${Number(r.memory_mib || 0)} MiB`,
      r.gpu_slots ? `${r.gpu_slots} GPU` : "",
      r.gpu_memory_mib ? `${r.gpu_memory_mib} MiB ${VRouterI18n.t("显存")}` : "",
    ].filter(Boolean);
    return `<span class="pill kind">${VRouterI18n.t("手动")}</span><div class="hint mono">${esc(parts.join(" · "))}</div>`;
  },

  budgetSummary(resources) {
    const r = resources || {};
    return [
      `${Number(r.cpu_millicores || 0)} mCPU`,
      `${Number(r.memory_mib || 0)} MiB`,
      Number(r.gpu_slots || 0) ? `${Number(r.gpu_slots)} GPU` : "",
      Number(r.gpu_memory_mib || 0) ? `${Number(r.gpu_memory_mib)} MiB ${VRouterI18n.t("显存")}` : "",
    ].filter(Boolean).join(" · ");
  },

  openBudgetDialog(qualified, upstreamId) {
    const model = this.state.models.find((item) => item.qualified === qualified && (!upstreamId || item.upstream_id === upstreamId));
    if (!model) return;
    const budget = this.budgetFor(model);
    const generic = model.upstream_source !== "v-scheduler";
    const resources = budget?.resources || budget?.default_resources || {};
    const limits = budget?.limits || budget?.default_limits || {};
    this.budgetGeneric = generic ? { upstream_url: model.upstream_url } : null;
    this.budgetTarget = { qualified, upstreamId };
    const automatic = budget?.default_resources || budget?.resources || null;
    $("budgetDefault").textContent = automatic
      ? `${VRouterI18n.t("自动默认预算")}：${this.budgetSummary(automatic)}${budget?.capacity ? ` · ${VRouterI18n.t("节点容量")} ${this.budgetSummary(budget.capacity)}` : ""}`
      : VRouterI18n.t("该模型没有自动默认预算（由上游直接转发），请按实测结果填写。");
    $("budgetRestore").hidden = !budget || budget.mode === "automatic";
    $("budgetModel").value = generic ? model.id : qualified;
    $("budgetModelName").textContent = model.qualified;
    $("budgetMode").querySelector('option[value="direct"]').disabled = generic;
    $("budgetMode").value = budget?.mode === "direct" && !generic ? "direct" : "calibrated";
    $("budgetCpu").value = Number(resources.cpu_millicores || 0) || "";
    $("budgetMemory").value = Number(resources.memory_mib || 0) || "";
    $("budgetGpuSlots").value = Number(resources.gpu_slots || 0);
    $("budgetGpuMemory").value = Number(resources.gpu_memory_mib || 0);
    $("budgetMaxTokens").value = Number(limits.max_tokens || 0) || "";
    $("budgetNumCtx").value = Number(limits["options.num_ctx"] || 0) || "";
    $("budgetNumPredict").value = Number(limits["options.num_predict"] || 0) || "";
    $("budgetNumThread").value = Number(limits["options.num_thread"] || 0) || "";
    $("budgetNumBatch").value = Number(limits["options.num_batch"] || 0) || "";
    $("budgetNumGpu").value = Number(limits["options.num_gpu"] || 0);
    $("budgetConfirm").checked = false;
    $("budgetError").textContent = "";
    this.updateBudgetMode();
    $("budgetDialog").showModal();
  },

  updateBudgetMode() {
    const direct = $("budgetMode").value === "direct";
    $("budgetResources").hidden = direct;
    $("budgetLimits").hidden = direct || Boolean(this.budgetGeneric);
    $("budgetDirectNote").hidden = !direct;
  },

  async saveBudget(event) {
    event.preventDefault();
    const model = $("budgetModel").value;
    const mode = $("budgetMode").value;
    $("budgetError").textContent = "";
    if (!$("budgetConfirm").checked) {
      $("budgetError").textContent = VRouterI18n.t("请先确认已了解该修改的影响");
      return false;
    }
    try {
      let body;
      if (mode === "direct") {
        body = { model_id: model, mode: "direct" };
      } else {
        const resources = {
          cpu_millicores: Number($("budgetCpu").value),
          memory_mib: Number($("budgetMemory").value),
          gpu_slots: Number($("budgetGpuSlots").value || 0),
          gpu_memory_mib: Number($("budgetGpuMemory").value || 0),
        };
        if (!(resources.cpu_millicores > 0) || !(resources.memory_mib > 0)) {
          throw new Error(VRouterI18n.t("CPU 和内存预算必须大于 0"));
        }
        if (resources.gpu_memory_mib > 0 && !(resources.gpu_slots > 0)) {
          throw new Error(VRouterI18n.t("设置显存预算时必须设置 GPU 槽位"));
        }
        body = { model_id: model, mode: "calibrated", resources };
        if (this.budgetGeneric) {
          body.upstream_url = this.budgetGeneric.upstream_url;
        } else {
          const limits = {
            max_tokens: Number($("budgetMaxTokens").value),
            "options.num_ctx": Number($("budgetNumCtx").value),
            "options.num_predict": Number($("budgetNumPredict").value),
            "options.num_thread": Number($("budgetNumThread").value),
            "options.num_batch": Number($("budgetNumBatch").value),
            "options.num_gpu": Number($("budgetNumGpu").value || 0),
          };
          for (const [key, value] of Object.entries(limits)) {
            if (!Number.isInteger(value) || value < 0 || (key !== "options.num_gpu" && value === 0)) {
              throw new Error(VRouterI18n.t("运行上限必须是正数"));
            }
          }
          body.limits = limits;
        }
      }
      await this.api("POST", "/scheduler/api/v1/model-budgets/calibrate", body);
      $("budgetDialog").close();
      this.toast(mode === "direct" ? VRouterI18n.t("该模型已直接放行，不再参与调度准入") : VRouterI18n.t("模型预算已校准"));
      await this.refresh();
    } catch (err) {
      $("budgetError").textContent = err.message;
    }
    return false;
  },

  async setModelDirect(upstreamId, native, direct) {
    try {
      await this.api("PUT", "/api/v1/model-routing", { model_id: `${upstreamId}/${native}`, direct });
      this.toast(direct ? VRouterI18n.t("该模型已直接放行，不再参与调度准入") : VRouterI18n.t("该模型已恢复调度准入"));
      await this.refresh();
    } catch (err) {
      this.toast(VRouterI18n.t("操作失败：") + err.message);
    }
  },

  async restoreBudget(qualified, upstreamId) {
    const model = this.state.models.find((item) => item.qualified === qualified && (!upstreamId || item.upstream_id === upstreamId));
    if (!model) return false;
    if (!confirm(`${VRouterI18n.t("恢复默认预算")} ${qualified}?`)) return false;
    const generic = model.upstream_source !== "v-scheduler";
    try {
      await this.api("POST", "/scheduler/api/v1/model-budgets/restore", generic
        ? { model_id: model.id, upstream_url: model.upstream_url }
        : { model_id: qualified });
      this.toast(VRouterI18n.t("已恢复默认预算"));
      await this.refresh();
      return true;
    } catch (err) {
      this.toast(VRouterI18n.t("恢复失败：") + err.message);
      return false;
    }
  },

  async restoreBudgetFromDialog() {
    if (!this.budgetTarget) return;
    const restored = await this.restoreBudget(this.budgetTarget.qualified, this.budgetTarget.upstreamId);
    if (restored) $("budgetDialog").close();
  },

  openModelDialog(qualified, upstreamId) {
    const model = this.state.models.find((item) => item.qualified === qualified && (!upstreamId || item.upstream_id === upstreamId));
    if (!model) return;
    $("modelEditName").textContent = model.qualified;
    $("modelEditQualified").value = model.qualified;
    $("modelEditUpstream").value = model.upstream_id || "";
    $("modelEditKind").value = ["chat", "embedding", "unknown"].includes(model.kind) ? model.kind : "";
    $("modelEditCapabilities").value = (model.capabilities || []).join(", ");
    $("modelEditEndpoints").value = (model.endpoints || []).join(", ");
    $("modelEditError").textContent = "";
    $("modelDialog").showModal();
  },

  async saveModelMetadata(event) {
    event.preventDefault();
    $("modelEditError").textContent = "";
    try {
      const payload = {
        model: $("modelEditQualified").value,
        upstream_id: $("modelEditUpstream").value,
        capabilities: splitList($("modelEditCapabilities").value),
        endpoints: splitList($("modelEditEndpoints").value),
      };
      const kind = $("modelEditKind").value;
      if (kind) payload.kind = kind;
      await this.api("PATCH", "/api/v1/models", payload);
      $("modelDialog").close();
      this.toast(VRouterI18n.t("模型元数据已保存"));
      await this.refresh();
    } catch (err) {
      $("modelEditError").textContent = err.message;
    }
    return false;
  },

  renderUpstreams() {
    const ups = this.state.upstreams;
    VRouterI18n.bind($("upstreamsTable"), "innerHTML", () => ups.length
      ? table(
          [VRouterI18n.t("名称"), VRouterI18n.t("类型"), VRouterI18n.t("来源"), VRouterI18n.t("模型"), VRouterI18n.t("状态"), VRouterI18n.t("操作")],
          ups.map((u) => {
            const manual = u.source === "api";
            const registered = u.source === "registered";
            const editable = u.manageable !== false;
            const scope = (u.owner || u.visibility === "private")
              ? ` · ${u.visibility === "private" ? "Private" : "Shared"}${u.owner ? ` · owner ${esc(u.owner)}` : ""}`
              : (registered ? " · Public" : "");
            return [
              `<span class="mono" title="${esc(u.base_url)}">${esc(u.name)}</span><div class="hint mono">ID: ${esc(u.id)} · ${esc(u.protocol || (u.source === "model-hub" ? "chat_completions" : "openai"))}${u.model_prefix ? ` · ${VRouterI18n.t("前缀")} ${esc(u.model_prefix)}` : ""}${scope}</div><div class="hint">${u.routing?.direct ? `${VRouterI18n.t("进行中")} ${u.inflight || 0} · ${VRouterI18n.t("直接放行")}` : `${VRouterI18n.t("进行中")} ${u.inflight || 0} / ${u.routing?.max_inflight || VRouterI18n.t("不限")} · ${VRouterI18n.t("权重")} ${u.routing?.weight || 1}`}${u.routing?.disabled ? VRouterI18n.t(" · 已暂停") : ""}</div>`,
              `<span class="pill ${u.kind}">${kindLabel(u.kind)}</span>`,
              u.source === "api" ? (u.kind === "cloud" ? VRouterI18n.t("云端 API") : VRouterI18n.t("手动添加")) : u.source === "registered" ? VRouterI18n.t("后端注册") : u.source === "video-structure" ? VRouterI18n.t("视频结构化") : u.source === "v-scheduler" ? "V-Scheduler" : "Model Hub",
              u.models,
              this.upstreamStatusPill(u),
              (editable
                ? `<button class="btn" onclick="app.openRouting('${esc(u.id)}')">${VRouterI18n.t("调度")}</button>`
                : `<span class="muted" style="font-size:12px">${VRouterI18n.t("仅")} owner/${VRouterI18n.t("管理员可操作")}</span>`) + (manual && editable
                ? `<div class="actions">` +
                  `<button class="btn" onclick="app.openUpstreamDialog('${esc(u.id)}')">${VRouterI18n.t("编辑")}</button>` +
                  `<button class="btn" onclick="app.refreshUpstream('${esc(u.id)}')">${VRouterI18n.t("刷新")}</button>` +
                  `<button class="btn danger" onclick="app.deleteUpstream('${esc(u.id)}', '${esc(u.name)}')">${VRouterI18n.t("删除")}</button></div>`
                : registered && editable
                  ? `<div class="actions"><button class="btn danger" onclick="app.unregisterService('${esc(u.id)}', '${esc(u.name)}')">${VRouterI18n.t("注销")}</button></div><span class="muted" style="font-size:12px">${VRouterI18n.t("后端租约注册")}${u.scheduler?.mode === "direct" ? ` · ${VRouterI18n.t("直接放行")}` : ""}</span>`
                  : registered
                    ? `<span class="muted" style="font-size:12px">${VRouterI18n.t("后端租约注册")}${u.scheduler?.mode === "direct" ? ` · ${VRouterI18n.t("直接放行")}` : ""}</span>`
                  : u.source === "video-structure"
                    ? `<span class="muted" style="font-size:12px">${VRouterI18n.t("自动发现")} · ${VRouterI18n.t("只读")}</span>`
                    : u.source === "v-scheduler"
                      ? `<span class="muted" style="font-size:12px">V-Scheduler · ${VRouterI18n.t("内置后端")}</span>`
                      : `<span class="muted" style="font-size:12px">${VRouterI18n.t("由")} Model Hub ${VRouterI18n.t("自动管理")}</span>`),
            ];
          }),
          [false, false, false, true, false, false],
        )
      : `<div class="empty">${VRouterI18n.t("暂无上游")}</div>`);
  },

  upstreamStatusPill(u) {
    const state = (u.status && u.status.state) || (u.healthy ? "healthy" : "discovery_failed");
    const meta = {
      healthy: [VRouterI18n.t("健康"), "ok"],
      paused: [VRouterI18n.t("已暂停"), "warn"],
      lease_expired: [VRouterI18n.t("租约过期"), "bad"],
      circuit_open: [VRouterI18n.t("熔断中"), "bad"],
      discovery_failed: [VRouterI18n.t("发现失败"), "bad"],
      auth_failed: [VRouterI18n.t("鉴权失败"), "bad"],
      no_models: [VRouterI18n.t("无模型"), "warn"],
      full: [VRouterI18n.t("满载"), "warn"],
    }[state] || [VRouterI18n.t("异常"), "bad"];
    const title = (u.status && u.status.detail) || u.last_error || meta[0];
    const until = state === "circuit_open" && u.status && u.status.circuit_open_until ? ` · ${new Date(u.status.circuit_open_until).toLocaleTimeString()} ${VRouterI18n.t("恢复")}` : "";
    return `<span class="pill ${meta[1]}" title="${esc(title)}">${meta[0]}${until}</span>`;
  },

  renderPlaygroundOptions() {
    const previous = $("pgModel").value;
    const usable = this.state.models
      .filter((m) => !m.short && m.kind !== "service" && this.playgroundEndpointsForModel(m).length)
      .sort((a, b) => this.playgroundModelRank(a.qualified) - this.playgroundModelRank(b.qualified) || a.qualified.localeCompare(b.qualified));
    VRouterI18n.bind($("pgModel"), "innerHTML", () => usable.map(m => `<option value="${esc(m.qualified)}">${esc(m.qualified)}</option>`).join("") || `<option value="">（${VRouterI18n.t("无可用模型")}）</option>`);
    if ([...$("pgModel").options].some(o => o.value === previous)) $("pgModel").value = previous;
    this.renderPlaygroundEndpoints();
  },
  renderPlaygroundEndpoints() {
    const previous = $("pgEndpoint").value;
    const endpoints = this.playgroundEndpointsForValue($("pgModel").value);
    VRouterI18n.bind($("pgEndpoint"), "innerHTML", () => endpoints.map(e => `<option value="${esc(e.id)}">${esc(e.label)}</option>`).join("") || `<option value="">（${VRouterI18n.t("无可用接口")}）</option>`);
    if (endpoints.some(e => e.id === previous)) $("pgEndpoint").value = previous;
    this.renderPlaygroundUpstreams();
    this.updatePlaygroundMode();
  },
  renderPlaygroundUpstreams() {
    const value = $("pgModel").value, endpoint = this.playgroundEndpoint();
    const previous = $("pgUpstream").value;
    const candidates = this.playgroundCandidates(value, endpoint);
    VRouterI18n.bind($("pgUpstream"), "innerHTML", () => candidates.map(u => `<option value="${esc(u.id)}">${esc(u.name)} · ${esc(u.id)}</option>`).join("") || `<option value="">${VRouterI18n.t("无可用上游")}</option>`);
    if (candidates.some(u => u.id === previous)) $("pgUpstream").value = previous;
  },
  updatePlaygroundMode() {
    const endpoint = this.playgroundEndpoint();
    const isEmbedding = endpoint?.id === "embeddings";
    $("pgStream").disabled = !endpoint?.stream;
    $("pgStream").checked = !!endpoint?.stream && $("pgStream").checked;
    VRouterI18n.bind($("pgInput"), "placeholder", () => isEmbedding ? VRouterI18n.t("输入要向量化的文本，Enter 发送") : VRouterI18n.t("输入消息，Enter 发送，Shift+Enter 换行"));
  },
  playgroundModelRank(value) {
    const endpoints = this.playgroundEndpointsForValue(value);
    if (endpoints.some(e => ["chat", "responses", "messages", "completions"].includes(e.id))) return 0;
    if (endpoints.some(e => e.id === "embeddings")) return 1;
    return 2;
  },
  playgroundEndpoint() {
    return PG_ENDPOINTS.find(e => e.id === $("pgEndpoint").value) || null;
  },
  playgroundEndpointsForValue(value) {
    return PG_ENDPOINTS.filter(e => this.playgroundCandidates(value, e).length);
  },
  playgroundEndpointsForModel(model) {
    return PG_ENDPOINTS.filter(e => this.playgroundCandidates(model.qualified, e).length);
  },
  playgroundCandidates(value, endpoint) {
    if (!value || !endpoint) return [];
    const rows = this.state.models.filter(m => !m.short && m.qualified === value);
    const seen = new Set();
    const out = [];
    for (const row of rows) {
      const native = row.id || value;
      for (const u of this.state.upstreams) {
        if (seen.has(u.id) || !u.healthy || u.routing?.disabled || !u.model_ids?.includes(native)) continue;
        if (this.upstreamSupportsEndpoint(u, row, endpoint.path)) {
          seen.add(u.id);
          out.push(u);
        }
      }
    }
    return out;
  },
  upstreamSupportsEndpoint(u, model, path) {
    const source = u.source || (u.kind === "local" ? "model-hub" : "");
    if (["model-hub", "v-scheduler"].includes(source)) {
      const role = (u.role || "").toLowerCase();
      if (role === "rerank" && path !== "/rerank") return false;
      if (path === "/rerank" && ["generate", "embed", "embedding"].includes(role)) return false;
    }
    const chatOnly = u.protocol === "chat_completions" || (!u.protocol && source === "model-hub");
    if (path === "/messages") {
      if (!chatOnly && u.protocol !== "anthropic") return false;
    } else if (u.protocol === "anthropic") {
      return false;
    }
    const endpoints = (model.endpoints || []).map(e => this.normalizeEndpoint(e));
    if (endpoints.length) {
      const wanted = this.normalizeEndpoint(path);
      if (endpoints.includes(wanted)) return true;
      if (endpoints.includes("chat_completions") && path === "/chat/completions") return true;
      if (endpoints.includes("chat_completions") && chatOnly && ["/responses", "/messages"].includes(path)) return true;
      return false;
    }
    const role = (u.role || "").toLowerCase();
    const knownCaps = this.modelHasKnownCapability(model);
    if (["/chat/completions", "/responses", "/messages", "/completions"].includes(path)) {
      if (["model-hub", "v-scheduler"].includes(source) && ["embed", "embedding"].includes(role)) return false;
      if (knownCaps) return this.modelHasAnyCapability(model, ["chat", "completion", "completions", "generate", "generation", "tools", "thinking"]);
      return model.kind !== "embedding";
    }
    if (path === "/embeddings") {
      if (["model-hub", "v-scheduler"].includes(source) && role === "generate") return false;
      if (knownCaps) return this.modelHasAnyCapability(model, ["embedding", "embeddings"]);
      return model.kind !== "chat";
    }
    return !chatOnly;
  },
  normalizeEndpoint(endpoint) {
    const e = String(endpoint || "").trim().toLowerCase().replace(/^\/v1\//, "").replace(/^\//, "").replaceAll("-", "_").replaceAll("/", "_");
    if (["chat", "chat_completions", "chat_completions"].includes(e)) return "chat_completions";
    if (["response", "responses"].includes(e)) return "responses";
    if (["message", "messages", "anthropic_messages"].includes(e)) return "messages";
    if (["completion", "completions"].includes(e)) return "completions";
    if (["embedding", "embeddings"].includes(e)) return "embeddings";
    return e.replaceAll("_", "/");
  },
  modelHasKnownCapability(model) {
    return (model.capabilities || []).some(c => ["embedding", "embeddings", "chat", "completion", "completions", "generate", "generation", "tools", "thinking", "image", "images", "image_generation", "image_edit", "image_variation", "video", "videos", "video_generation", "asr", "tts", "speech", "translation", "moderation", "rerank"].includes(String(c).trim().toLowerCase()));
  },
  modelHasAnyCapability(model, names) {
    const wanted = new Set(names);
    return (model.capabilities || []).some(c => wanted.has(String(c).trim().toLowerCase()));
  },
  openRouting(id) {
    const u = this.state.upstreams.find(u => u.id === id);
    if (!u) return;
    $("routingId").value = id; $("routingWeight").value = u.routing?.weight || 1;
    $("routingMax").value = u.routing?.max_inflight || 0; $("routingDisabled").checked = !!u.routing?.disabled;
    $("routingDirect").checked = !!u.routing?.direct;
    const managed = u.source !== "api";
    $("routingDirect").disabled = managed && this.state.status?.is_admin !== true;
    VRouterI18n.bind($("routingDirectHint"), "textContent", () => managed
      ? VRouterI18n.t("该上游由 V-Scheduler 或应用托管；直接放行会让其下所有模型跳过模型预算与调度准入，仅管理员可设置。")
      : VRouterI18n.t("直接放行会跳过并发上限、熔断、模型预算与调度准入，并在有多个可用上游时优先选择；暂停仍然生效。外部云端 API 通常应直接放行。"));
    $("routingError").textContent = ""; this.updateRoutingDirect(); $("routingDialog").showModal();
  },
  updateRoutingDirect() {
    const direct = $("routingDirect").checked;
    $("routingWeight").disabled = direct; $("routingMax").disabled = direct;
  },
  async saveRouting(e) {
    e.preventDefault();
    try {
      await this.api("PUT", "/api/v1/upstreams/" + encodeURIComponent($("routingId").value) + "/routing", {
        weight: Number($("routingWeight").value), max_inflight: Number($("routingMax").value), disabled: $("routingDisabled").checked, direct: $("routingDirect").checked
      });
      $("routingDialog").close(); await this.refresh(); this.toast(VRouterI18n.t("调度配置已保存"));
    } catch(err) { $("routingError").textContent = err.message; }
    return false;
  },

  // ---------- about ----------
  openAbout() {
    const st = this.state.status;
    VRouterI18n.bind($("aboutBody"), "innerHTML", () => [
      [VRouterI18n.t("版本"), st?.version ? "v" + st.version : VRouterI18n.t("未知")],
      [VRouterI18n.t("应用 ID"), "com.ictrek.v-router"],
      [VRouterI18n.t("VOS 内调用"), "http://v-scheduler:8080/v1"],
      [VRouterI18n.t("局域网调用"), "http://<vos-host>:22888/v1"],
      [VRouterI18n.t("API 文档"), VRouterI18n.t("本页面「API 文档」页签，由网关实时生成")],
      [VRouterI18n.t("项目仓库"), "github.com/ictrektech/V-Scheduler"],
    ].map(([k, v]) => `<div class="about-row"><span class="k">${k}</span><span class="v">${esc(v)}</span></div>`).join(""));
    $("aboutDialog").showModal();
  },


  // ---------- registration dialog ----------
  openRegistrationDialog() {
    $("registrationForm").reset();
    $("regProtocol").value = "openai";
    $("regVisibility").value = "public";
    $("regMode").value = "api";
    $("regCapabilityPreset").value = "chat";
    $("regTTL").value = "90";
    $("regPreset").value = "";
    $("regScheduler").value = "scheduled";
    $("regSchedulerField").hidden = !this.state.status?.is_admin;
    $("regError").textContent = "";
    this.renderRegistrationResult(null);
    this.setRegistrationModelChoices([]);
    this.updateRegistrationMode();
    $("registrationDialog").showModal();
  },
  updateRegistrationMode() {
    const single = $("regMode").value === "single";
    $("regSingleFields").hidden = !single;
    if (single && !$("regEndpoints").value.trim()) this.applyRegistrationPreset();
  },
  updateRegistrationProtocol() {
    if ($("regProtocol").value === "offline_asr") {
      $("regMode").value = "single";
      $("regCapabilityPreset").value = "asr";
      this.applyRegistrationPreset();
    } else if ($("regProtocol").value === "image_studio") {
      if (!["image_generation", "image_edit", "video_generation", "custom"].includes($("regCapabilityPreset").value)) {
        $("regCapabilityPreset").value = "image_generation";
        this.applyRegistrationPreset();
      }
    } else if ($("regCapabilityPreset").value === "asr") {
      $("regCapabilityPreset").value = "chat";
      this.applyRegistrationPreset();
    }
    this.updateRegistrationMode();
  },
  applyRegistrationPreset() {
    const preset = $("regCapabilityPreset").value;
    if (preset === "custom") return;
    const presets = {
      chat: { endpoints: "chat_completions, responses, messages", caps: "chat, completion, tools", kind: "chat" },
      chat_completions: { endpoints: "chat_completions", caps: "chat, completion", kind: "chat" },
      responses: { endpoints: "responses", caps: "completion, tools", kind: "chat" },
      messages: { endpoints: "messages", caps: "chat, completion", kind: "chat" },
      completions: { endpoints: "completions", caps: "completion", kind: "chat" },
      embedding: { endpoints: "embeddings", caps: "embedding", kind: "embedding" },
      rerank: { endpoints: "rerank", caps: "rerank", kind: "unknown" },
      image_generation: { endpoints: "images/generations", caps: "image_generation", kind: "unknown" },
      image_edit: { endpoints: "images/edits", caps: "image_edit", kind: "unknown" },
      image_variation: { endpoints: "images/variations", caps: "image_variation", kind: "unknown" },
      video_generation: { endpoints: "videos/generations", caps: "video_generation", kind: "unknown" },
      asr: { endpoints: "audio/transcriptions", caps: "asr", kind: "unknown" },
      audio_translation: { endpoints: "audio/translations", caps: "translation", kind: "unknown" },
      tts: { endpoints: "audio/speech", caps: "tts", kind: "unknown" },
      moderation: { endpoints: "moderations", caps: "moderation", kind: "unknown" },
    };
    const p = presets[preset] || presets.chat;
    $("regEndpoints").value = p.endpoints;
    $("regCapabilities").value = p.caps;
    $("regModelKind").value = p.kind;
  },
  setRegistrationModelChoices(models) {
    const field = $("regModelPickField");
    const sel = $("regModelPick");
    VRouterI18n.bind(sel, "innerHTML", () => VRouterI18n.t('<option value="">选择一个模型转为单模型注册…</option>') + (models || []).map(m => `<option value="${esc(m)}">${esc(m)}</option>`).join(""));
    field.hidden = !(models && models.length);
  },
  pickRegistrationModel() {
    const model = $("regModelPick").value;
    if (!model) return;
    $("regModel").value = model;
    $("regMode").value = "single";
    if ($("regProtocol").value === "offline_asr") $("regCapabilityPreset").value = "asr";
    if ($("regProtocol").value === "image_studio" && !["image_generation", "image_edit", "video_generation", "custom"].includes($("regCapabilityPreset").value)) $("regCapabilityPreset").value = "image_generation";
    this.applyRegistrationPreset();
    this.updateRegistrationMode();
  },
  listFromInput(id) {
    return splitList($(id).value);
  },
  registrationPayload() {
    const p = {
      service_id: $("regServiceId").value.trim(),
      name: $("regName").value.trim(),
      visibility: $("regVisibility").value,
      protocol: $("regProtocol").value,
      base_url: $("regBaseURL").value.trim(),
      api_key: $("regAPIKey").value,
      ttl_seconds: Number($("regTTL").value || 90),
    };
    if (this.state.status?.is_admin && $("regScheduler").value === "direct") {
      p.scheduler = { mode: "direct" };
    }
    if ($("regMode").value === "single") {
      p.model = $("regModel").value.trim();
      p.endpoints = this.listFromInput("regEndpoints");
      p.capabilities = this.listFromInput("regCapabilities");
      p.model_kind = $("regModelKind").value || "unknown";
    }
    return p;
  },
  async testRegistration() {
    this.renderRegistrationResult({ pending: true });
    $("regError").textContent = "";
    const p = this.registrationPayload();
    try {
      if ($("regMode").value === "single") {
        if (!p.model || !p.endpoints?.length) throw new Error(VRouterI18n.t("单模型注册测试需要填写模型 ID 和支持接口"));
        this.renderRegistrationResult({ ok: true, models: [p.model], note: VRouterI18n.t("单模型声明完整；保存后会按 endpoints 过滤可访问接口。") });
        return;
      }
      const result = await this.api("POST", "/api/v1/upstreams/probe", { name: p.name || p.service_id || "registration", base_url: p.base_url, api_key: p.api_key, protocol: p.protocol, kind: "manual" });
      this.setRegistrationModelChoices(result.ok ? result.models || [] : []);
      this.renderRegistrationResult(result);
    } catch (err) {
      this.setRegistrationModelChoices([]);
      this.renderRegistrationResult({ ok: false, error: err.message });
    }
  },
  renderRegistrationResult(result) {
    const el = $("regProbeResult");
    if (!result) { el.hidden = true; el.className = "probe-result"; el.innerHTML = ""; return; }
    el.hidden = false;
    if (result.pending) { el.className = "probe-result"; VRouterI18n.bind(el, "textContent", () => VRouterI18n.t("正在测试…")); return; }
    el.className = "probe-result " + (result.ok ? "ok" : "bad");
    const models = result.models || [];
    VRouterI18n.bind(el, "innerHTML", () => result.ok
      ? `<strong>${VRouterI18n.t("测试通过")}</strong><div class="hint">${esc(result.note || (models.length ? `${VRouterI18n.t("发现")} ${models.length} ${VRouterI18n.t("个模型")}：${models.slice(0, 12).map(esc).join(", ")}${models.length > 12 ? " …" : ""}` : VRouterI18n.t("模型列表为空")))}</div><div class="hint">${VRouterI18n.t("注册整个")} API ${VRouterI18n.t("时模型默认按上游协议可用")}；${VRouterI18n.t("也可以在下拉里选择一个模型")}，${VRouterI18n.t("再按该模型的真实能力注册单模型")}。</div>`
      : `<strong>${VRouterI18n.t("测试未通过")}</strong><div class="hint">${esc(result.error || VRouterI18n.t("未知错误"))}</div><div class="hint">${VRouterI18n.t("仍可保存单模型声明")}；${VRouterI18n.t("模型")} API ${VRouterI18n.t("模式需要")} /models ${VRouterI18n.t("可访问")}。</div>`);
  },
  async submitRegistration(event) {
    event.preventDefault();
    $("regSubmit").disabled = true;
    $("regError").textContent = "";
    try {
      const result = await this.api("POST", "/api/v1/registrations", this.registrationPayload());
      $("registrationDialog").close();
      this.toast(`${VRouterI18n.t("已注册")} ${result.upstream?.id || VRouterI18n.t("模型服务")}`);
      await this.refresh();
    } catch (err) {
      $("regError").textContent = err.message;
    } finally {
      $("regSubmit").disabled = false;
    }
    return false;
  },
  async unregisterService(id, name) {
    if (!confirm(`${VRouterI18n.t("注销注册服务")}「${name}」？${VRouterI18n.t("它的模型会立即退出新请求分配")}。`)) return;
    try {
      await this.api("DELETE", `/api/v1/registrations/${encodeURIComponent(id)}`);
      this.toast(VRouterI18n.t("已注销"));
      await this.refresh();
    } catch (err) {
      this.toast(VRouterI18n.t("注销失败：") + err.message);
    }
  },

  // ---------- upstream dialog ----------
  openCloudDialog() {
    this.openUpstreamDialog(null, "cloud");
  },

  clearNewUpstreamFields() {
    for (const id of ["ufName", "ufBaseURL", "ufAPIKey", "ufTestModel", "ufAsyncModel", "ufAsyncCreatePath", "ufAsyncQueryPath", "ufAsyncTaskIDPath", "ufAsyncStatusPath", "ufAsyncResultPath", "ufAsyncResultJSONPath", "ufAsyncInputPath", "ufAsyncSuccessStates", "ufAsyncFailureStates", "ufAsyncDefaultInput"]) {
      const el = $(id);
      el.value = "";
      el.defaultValue = "";
    }
  },

  setUpstreamModels(models, { loaded = true, preserve = false } = {}) {
    // Keep selected rows and any metadata the user already edited across probes.
    const previous = preserve ? this.readModelRows() : new Map();
    const selectedBefore = preserve ? new Set(this.upstreamModelSelection()) : null;
    const allSelected = selectedBefore && this.modelChoices.length && selectedBefore.size === this.modelChoices.length;
    this.modelChoices = [];
    const seen = new Set();
    for (const raw of models || []) {
      const model = typeof raw === "string" ? { id: raw } : raw;
      if (!model || !model.id || seen.has(model.id)) continue;
      seen.add(model.id);
      const prior = previous.get(model.id);
      const kind = model.kind === "unknown" ? "" : model.kind || "";
      this.modelChoices.push({
        id: model.id,
        kind: prior ? prior.kind : kind,
        capabilities: prior ? prior.capabilities : (model.capabilities || []),
        endpoints: prior ? prior.endpoints : (model.endpoints || []),
      });
    }
    this.modelChoicesLoaded = loaded;
    const selected = selectedBefore && !allSelected ? new Set([...selectedBefore].filter((id) => seen.has(id))) : seen;
    $("ufModels").innerHTML = this.modelChoices.map((model) => `<div class="model-choice" data-model="${esc(model.id)}">
      <label class="check"><input type="checkbox" value="${esc(model.id)}" ${selected.has(model.id) ? "checked" : ""} onchange="app.updateUpstreamModelsHint()" /><span class="mono">${esc(model.id)}</span></label>
      <div class="model-meta">
        <select class="uf-model-kind" title="${esc(VRouterI18n.t("模型类型"))}">${modelKindOptions(model.kind)}</select>
        <input class="uf-model-caps" type="text" value="${esc((model.capabilities || []).join(", "))}" placeholder="${esc(VRouterI18n.t("能力（可选，逗号分隔）"))}" title="${esc(VRouterI18n.t("模型能力"))}" />
        <input class="uf-model-endpoints" type="text" value="${esc((model.endpoints || []).join(", "))}" placeholder="${esc(VRouterI18n.t("接口（可选，逗号分隔）"))}" title="${esc(VRouterI18n.t("支持接口"))}" />
      </div>
    </div>`).join("");
    $("ufModelsField").hidden = $("ufProtocol").value === "async_task" || !this.modelChoices.length;
    this.updateUpstreamModelsHint();
  },
  readModelRows() {
    const rows = new Map();
    for (const row of $("ufModels").querySelectorAll(".model-choice")) {
      rows.set(row.dataset.model, {
        kind: row.querySelector(".uf-model-kind")?.value || "",
        capabilities: splitList(row.querySelector(".uf-model-caps")?.value || ""),
        endpoints: splitList(row.querySelector(".uf-model-endpoints")?.value || ""),
      });
    }
    return rows;
  },
  upstreamModelSelection() {
    return [...$("ufModels").querySelectorAll('input[type="checkbox"]:checked')].map((el) => el.value);
  },
  upstreamModelPayload() {
    const rows = this.readModelRows();
    return this.upstreamModelSelection().map((id) => {
      const row = rows.get(id) || {};
      const item = { id };
      if (row.kind) item.kind = row.kind;
      if (row.capabilities?.length) item.capabilities = row.capabilities;
      if (row.endpoints?.length) item.endpoints = row.endpoints;
      return item;
    });
  },
  selectAllUpstreamModels(on) {
    for (const el of $("ufModels").querySelectorAll('input[type="checkbox"]')) el.checked = on;
    this.updateUpstreamModelsHint();
  },
  updateUpstreamModelsHint() {
    const selected = this.upstreamModelSelection().length;
    VRouterI18n.bind($("ufModelsHint"), "textContent", () => `${VRouterI18n.t("已选择")} ${selected} / ${this.modelChoices.length} ${VRouterI18n.t("个模型")}；${VRouterI18n.t("类型、能力和接口留空时按探测或默认值处理")}。`);
  },

  renderProviderPresets(select) {
    if (!select) return;
    select.innerHTML = `<option value="">${esc(VRouterI18n.t("自定义（手动填写）"))}</option>` +
      PROVIDER_PRESETS.map((p) => `<option value="${esc(p.id)}">${esc(p.name)}</option>`).join("");
  },
  applyProviderPreset(preset, fields) {
    if (!preset) return;
    $(fields.name).value = preset.name;
    $(fields.base).value = preset.base;
    $(fields.protocol).value = preset.protocol;
    $(fields.key).value = "";
    $(fields.key).placeholder = fields.keyPlaceholder || "sk-...";
    if (fields.model) {
      $(fields.model).value = "";
      $(fields.model).placeholder = `${VRouterI18n.t("例如")} ${preset.model}`;
    }
  },
  applyUpstreamPreset() {
    const preset = PROVIDER_PRESETS.find((p) => p.id === $("ufPreset").value);
    if (!preset) return;
    this.applyProviderPreset(preset, {
      name: "ufName",
      base: "ufBaseURL",
      protocol: "ufProtocol",
      key: "ufAPIKey",
      model: "ufTestModel",
    });
    this.setUpstreamModels([], { loaded: false });
    this.updateUpstreamProtocol();
  },
  applyRegistrationProviderPreset() {
    const preset = PROVIDER_PRESETS.find((p) => p.id === $("regPreset").value);
    if (!preset) return;
    this.applyProviderPreset(preset, {
      name: "regName",
      base: "regBaseURL",
      protocol: "regProtocol",
      key: "regAPIKey",
      keyPlaceholder: VRouterI18n.t("供应商或模型服务自己的 key"),
    });
    this.updateRegistrationProtocol();
    $("regModelPickField").hidden = true;
    $("regProbeResult").hidden = true;
  },

  openUpstreamDialog(id, kind = "manual") {
    const dlg = $("upstreamDialog");
    $("ufError").textContent = "";
    this.renderProbeResult(null);
    this.setUpstreamModels([], { loaded: false });
    if (id) {
      const u = this.state.upstreams.find((x) => x.id === id);
      if (!u) return;
      VRouterI18n.bind($("upstreamDialogTitle"), "textContent", () => u.kind === "cloud" ? VRouterI18n.t("编辑云端算力 API") : VRouterI18n.t("编辑上游"));
      $("ufId").value = u.id;
      $("ufKind").value = u.kind || "manual";
      $("ufPresetField").hidden = true;
      $("ufPreset").value = "";
      $("ufName").value = u.name;
      $("ufBaseURL").value = u.base_url;
      $("ufProtocol").value = u.protocol || "openai";
      $("ufVisibility").value = u.visibility === "private" ? "private" : "public";
      $("ufAPIKey").value = "";
      $("ufTestModel").value = "";
      $("ufAllowProbeFailure").checked = u.kind === "cloud";
      $("ufAsyncModel").value = (u.model_ids || [])[0] || "";
      const c = u.async_task || {};
      $("ufAsyncCreatePath").value = c.create_path || "";
      $("ufAsyncQueryPath").value = c.query_path || "";
      $("ufAsyncQueryParam").value = c.query_task_param || "taskId";
      $("ufAsyncTaskIDPath").value = c.task_id_path || "";
      $("ufAsyncStatusPath").value = c.status_path || "";
      $("ufAsyncResultPath").value = c.result_path || "";
      $("ufAsyncResultJSONPath").value = c.result_json_path || "";
      $("ufAsyncInputPath").value = c.input_path || "";
      $("ufAsyncSuccessStates").value = (c.success_states || []).join(", ");
      $("ufAsyncFailureStates").value = (c.failure_states || []).join(", ");
      $("ufAsyncDefaultInput").value = c.default_input ? JSON.stringify(c.default_input, null, 2) : "";
      $("ufAsyncPollMS").value = c.poll_interval_ms || 1000;
      $("ufAsyncTimeout").value = c.timeout_seconds || 300;
      const owned = this.state.models
        .filter((model) => model.upstream_id === u.id)
        .map((model) => ({ id: model.id, kind: model.kind, capabilities: model.capabilities, endpoints: model.endpoints }));
      this.setUpstreamModels(owned.length ? owned : (u.model_ids || []).map((model) => ({ id: model })));
      VRouterI18n.bind($("ufKeyHint"), "textContent", () => VRouterI18n.t("留空保持原密钥不变；密钥保存在本机，接口永不回显。"));
      VRouterI18n.bind($("ufBaseHint"), "innerHTML", () => u.kind === "cloud" ? this.cloudHintHTML(u.model_prefix) : VRouterI18n.t("可填 OpenAI 兼容 /v1、Anthropic API 地址；未写协议时默认 https，未写路径时默认 /v1。"));
    } else {
      $("upstreamForm").reset();
      this.clearNewUpstreamFields();
      $("ufId").value = "";
      $("ufKind").value = kind;
      $("ufPresetField").hidden = false;
      $("ufPreset").value = "";
      $("ufProtocol").value = "openai";
      $("ufVisibility").value = "public";
      VRouterI18n.bind($("ufKeyHint"), "textContent", () => VRouterI18n.t("密钥保存在本机状态文件中，接口永不回显。"));
      if (kind === "cloud") {
        VRouterI18n.bind($("upstreamDialogTitle"), "textContent", () => VRouterI18n.t("添加云端算力 API"));
        $("ufName").placeholder = "ICTrek AI";
        $("ufBaseURL").placeholder = "https://ai.ictrek.com/v1";
        $("ufAllowProbeFailure").checked = true;
        $("ufBaseHint").innerHTML = this.cloudHintHTML("com.ictrek.ai");
      } else {
        VRouterI18n.bind($("upstreamDialogTitle"), "textContent", () => VRouterI18n.t("添加手动上游"));
        VRouterI18n.bind($("ufName"), "placeholder", () => VRouterI18n.t("例如 ICTrek AI / OpenAI / Anthropic"));
        $("ufBaseURL").placeholder = "https://api.example.com/v1";
        $("ufAllowProbeFailure").checked = false;
        VRouterI18n.bind($("ufBaseHint"), "textContent", () => VRouterI18n.t("可填 OpenAI 兼容 /v1、Anthropic API 地址；未写协议时默认 https，未写路径时默认 /v1。"));
      }
    }
    this.updateUpstreamProtocol();
    dlg.showModal();
    if (!id) {
      requestAnimationFrame(() => {
        this.clearNewUpstreamFields();
      });
    }
  },

  cloudHintHTML(prefix) {
    return `${VRouterI18n.t("默认")} ICTrek ${VRouterI18n.t("云端算力地址为")} <span class="mono">https://ai.ictrek.com/v1</span>；<a href="https://ai.ictrek.com" target="_blank" rel="noreferrer">${VRouterI18n.t("去")} ai.ictrek.com ${VRouterI18n.t("注册")}/${VRouterI18n.t("购买")}</a>。${VRouterI18n.t("模型前缀按域名生成")}${prefix ? `，${VRouterI18n.t("当前为")} <span class="mono">${esc(prefix)}</span>` : ""}。`;
  },

  closeUpstreamDialog() {
    $("upstreamDialog").close();
  },

  updateUpstreamProtocol() {
    const asyncTask = $("ufProtocol").value === "async_task";
    $("ufAsyncFields").hidden = !asyncTask;
    $("ufTestModel").closest(".field").hidden = asyncTask;
    $("ufAllowProbeFailure").closest(".field").hidden = asyncTask;
    $("ufModelsField").hidden = asyncTask || !this.modelChoices.length;
    VRouterI18n.bind($("ufProbe"), "textContent", () => asyncTask ? VRouterI18n.t("检查配置") : VRouterI18n.t("测试"));
  },

  upstreamPayload() {
    const kind = $("ufKind").value || "manual";
    const isCloud = kind === "cloud";
    const payload = {
      kind,
      name: $("ufName").value.trim() || (isCloud ? "ICTrek AI" : ""),
      base_url: $("ufBaseURL").value.trim() || (isCloud ? "https://ai.ictrek.com/v1" : ""),
      api_key: $("ufAPIKey").value,
      protocol: $("ufProtocol").value,
      visibility: $("ufVisibility").value,
      test_model: $("ufTestModel").value.trim(),
      allow_probe_failure: $("ufAllowProbeFailure").checked,
    };
    if (this.modelChoicesLoaded && payload.protocol !== "async_task") {
      payload.models = this.upstreamModelPayload();
    }
    if (payload.protocol === "async_task") {
      let defaultInput = {};
      const raw = $("ufAsyncDefaultInput").value.trim();
      if (raw) {
        try { defaultInput = JSON.parse(raw); }
        catch (_) { throw new Error(VRouterI18n.t("默认输入参数必须是有效的 JSON 对象")); }
        if (!defaultInput || Array.isArray(defaultInput) || typeof defaultInput !== "object") throw new Error(VRouterI18n.t("默认输入参数必须是 JSON 对象"));
      }
      payload.model = $("ufAsyncModel").value.trim();
      payload.async_task = {
        create_path: $("ufAsyncCreatePath").value.trim(),
        query_path: $("ufAsyncQueryPath").value.trim(),
        query_task_param: $("ufAsyncQueryParam").value.trim() || "taskId",
        task_id_path: $("ufAsyncTaskIDPath").value.trim(),
        status_path: $("ufAsyncStatusPath").value.trim(),
        result_path: $("ufAsyncResultPath").value.trim(),
        result_json_path: $("ufAsyncResultJSONPath").value.trim(),
        input_path: $("ufAsyncInputPath").value.trim(),
        success_states: this.listFromInput("ufAsyncSuccessStates"),
        failure_states: this.listFromInput("ufAsyncFailureStates"),
        default_input: defaultInput,
        poll_interval_ms: Number($("ufAsyncPollMS").value || 1000),
        timeout_seconds: Number($("ufAsyncTimeout").value || 300),
      };
    }
    return payload;
  },

  async probeUpstream() {
    $("ufProbe").disabled = true;
    $("ufError").textContent = "";
    this.renderProbeResult({ pending: true });
    try {
      const payload = this.upstreamPayload();
      if ($("ufId").value) payload.upstream_id = $("ufId").value;
      const result = await this.api("POST", "/api/v1/upstreams/probe", payload);
      if (result.ok) this.setUpstreamModels(result.model_details || result.models || [], { preserve: true });
      this.renderProbeResult(result);
    } catch (err) {
      this.renderProbeResult({ ok: false, error: err.message });
    } finally {
      $("ufProbe").disabled = false;
    }
  },

  renderProbeResult(result) {
    const el = $("ufProbeResult");
    if (!result) { el.hidden = true; el.className = "probe-result"; el.innerHTML = ""; return; }
    el.hidden = false;
    if (result.pending) { el.className = "probe-result"; VRouterI18n.bind(el, "textContent", () => VRouterI18n.t("正在测试…")); return; }
    el.className = "probe-result " + (result.ok ? "ok" : "bad");
    const models = result.models || [];
    VRouterI18n.bind(el, "innerHTML", () => result.ok
      ? `<strong>${VRouterI18n.t("模型列表可用")}</strong><div class="hint mono">${VRouterI18n.t("前缀")} ${esc(result.model_prefix || "")} · ${esc(result.base_url || "")}</div><div class="hint">${models.length ? `${VRouterI18n.t("发现")} ${models.length} ${VRouterI18n.t("个模型")}：${esc(models.slice(0, 12).join(", "))}${models.length > 12 ? " …" : ""}` : VRouterI18n.t("模型列表为空")}</div>${result.test_ok ? `<div class="hint">${VRouterI18n.t("指定模型联通测试通过")}</div>` : result.test_error ? `<div class="hint">${VRouterI18n.t("指定模型测试失败")}：${esc(result.test_error)}</div>` : ""}`
      : `<strong>${VRouterI18n.t("模型列表测试未通过")}</strong><div class="hint">${esc(result.error || VRouterI18n.t("未知错误"))}</div>${result.test_ok ? `<div class="hint">${VRouterI18n.t("但指定模型联通测试通过")}；${VRouterI18n.t("保存时会把该模型加入列表")}。</div>` : result.test_error ? `<div class="hint">${VRouterI18n.t("指定模型测试也失败")}：${esc(result.test_error)}</div>` : ""}<div class="hint">${VRouterI18n.t("仍可勾选")}“${VRouterI18n.t("探测失败也允许保存")}”，${VRouterI18n.t("让运行时返回上游的具体错误")}。</div>`);
  },

  async submitUpstream(event) {
    event.preventDefault();
    const id = $("ufId").value;
    $("ufSubmit").disabled = true;
    $("ufError").textContent = "";
    try {
      const payload = this.upstreamPayload();
      if (id) await this.api("PUT", `/api/v1/upstreams/${encodeURIComponent(id)}`, payload);
      else await this.api("POST", "/api/v1/upstreams", payload);
      this.closeUpstreamDialog();
      this.toast(id ? VRouterI18n.t("上游已更新") : VRouterI18n.t("上游已添加"));
      await this.refresh();
    } catch (err) {
      $("ufError").textContent = err.message;
    } finally {
      $("ufSubmit").disabled = false;
    }
    return false;
  },

  async refreshUpstream(id) {
    try {
      await this.api("POST", `/api/v1/upstreams/${encodeURIComponent(id)}/refresh`);
      this.toast(VRouterI18n.t("已刷新"));
      await this.refresh();
    } catch (err) {
      this.toast(VRouterI18n.t("刷新失败：") + err.message);
    }
  },

  async deleteUpstream(id, name) {
    if (!confirm(`${VRouterI18n.t("删除上游")}「${name}」？${VRouterI18n.t("其下模型将立即从网关移除")}。`)) return;
    try {
      await this.api("DELETE", `/api/v1/upstreams/${encodeURIComponent(id)}`);
      this.toast(VRouterI18n.t("已删除"));
      await this.refresh();
    } catch (err) {
      this.toast(VRouterI18n.t("删除失败：") + err.message);
    }
  },

  tryModel(qualified) {
    this.switchTab("playground");
    setTimeout(() => {
      $("pgModel").value = qualified;
      this.renderPlaygroundEndpoints();
      $("pgInput").focus();
    }, 0);
  },

  // ---------- api docs ----------
  // Rendered live from GET /api/v1/apidoc, which the backend generates from
  // the same route table that registers its handlers — the page always shows
  // exactly what the running version serves.
  async renderApiDoc() {
    const container = $("apidocGroups");
    const request = this.apiDocRequest = (this.apiDocRequest || 0) + 1;
    const lang = VRouterI18n.locale === "en" ? "en" : "zh-CN";

    // Clear the previous-language payload immediately while the new one loads.
    VRouterI18n.bind($("apidocIdentity"), "innerHTML", () => "");
    VRouterI18n.bind(container, "innerHTML", () => `<div class="empty">${VRouterI18n.t("文档加载中…")}</div>`);
    let doc;
    try {
      doc = await this.api("GET", `/api/v1/apidoc?lang=${lang}`);
      if (request !== this.apiDocRequest) return;
    } catch (err) {
      if (request !== this.apiDocRequest) return;
      VRouterI18n.bind(container, "innerHTML", () => `<div class="empty">${VRouterI18n.t("文档加载失败")}：${esc(VRouterI18n.t(err.sourceMessage || err.message))}</div>`);
      return;
    }

    const bases = doc.base_urls || {};
    VRouterI18n.bind($("apidocIntro"), "innerHTML", () => table(
      [VRouterI18n.t("调用方"), VRouterI18n.t("地址")],
      [
        [VRouterI18n.t("VOS 内应用（正式地址）"), `<span class="mono">${esc(bases.internal)}</span><div class="hint">${VRouterI18n.t("容器网络直连")}，${VRouterI18n.t("内网免鉴权")}；${VRouterI18n.t("携带")} fastpath token ${VRouterI18n.t("则按用户归集")}</div>`],
        [VRouterI18n.t("VOS 内应用（兼容地址）"), (bases.compatibility_aliases || []).map(url => `<div class="mono">${esc(url)}</div>`).join("") + `<div class="hint">${VRouterI18n.t("正式地址统一使用 v-scheduler；其他名称均为兼容地址。新应用和已有应用后续适配均推荐使用 v-scheduler。")}</div>`],
        [VRouterI18n.t("局域网客户端"), `<span class="mono">${esc(lanBaseURL(bases))}</span><div class="hint">host-port ${VRouterI18n.t("为安装参数")} V_ROUTER_HOST_PORT（${VRouterI18n.t("默认")} 22888）</div>`],
        [VRouterI18n.t("同域页面 / 其他 VOS 应用前端"), `<span class="mono">${esc(bases.same_origin)}</span><div class="hint">${VRouterI18n.t("携带自身")} VOS fastpath token ${VRouterI18n.t("即可")}，${VRouterI18n.t("无需向")} V-Router ${VRouterI18n.t("注册")}</div>`],
        [
          VRouterI18n.t("鉴权"),
          doc.key_required
            ? VRouterI18n.t("已启用身份校验：请求可携带有效 VOS token 或 <span class='mono'>Authorization: Bearer &lt;key&gt;</span>")
            : VRouterI18n.t("内网免鉴权；宿主机端口暴露到不可信网络时建议在安装参数中开启"),
        ],
      ],
    ) + `<p class="hint">${VRouterI18n.t("多个 host alias 是同一个容器的 DNS 名称，按原服务角色保留；不会增加容器。8080、38120、6005 分别是网关、调度和模型管理入口，三个 Ollama 实例各自使用独立容器。")}</p>` + `<div class="hint" style="margin-top:10px">${VRouterI18n.t("以下文档由运行中的网关实时生成")}（v${esc(doc.version)}），${VRouterI18n.t("与当前版本支持的接口保持一致")}。</div>`);

    VRouterI18n.bind($("apidocIdentity"), "innerHTML", () => (doc.identity || [])
      .map(
        (row) => `<div class="ep"><div class="ep-head"><span class="pill kind">${esc(row.channel)}</span></div>
          <div class="ep-desc"><span class="mono">${esc(row.method)}</span></div>
          <div class="ep-desc">${esc(row.effect)}</div></div>`,
      )
      .join("") + (doc.integration || []).map(row => `<div class="ep"><strong>${esc(row.title)}</strong><div class="ep-desc">${esc(row.note)}</div><pre class="ep-example" data-copy="${esc(row.example)}">${esc(row.example)}</pre><button class="btn" onclick="app.copyExample(this)">${VRouterI18n.t("复制示例")}</button></div>`).join("") + `<div class="hint" style="margin-top:6px">${VRouterI18n.t("用量按用户归集")}，${VRouterI18n.t("可在")}「${VRouterI18n.t("总览")} · ${VRouterI18n.t("按用户")}」${VRouterI18n.t("查看")}。${VRouterI18n.t("当前已登录用户共享上游管理权限与上游资源")}，${VRouterI18n.t("尚无个人")} Key ${VRouterI18n.t("或用户资源隔离")}。</div>`);

    const authLabel = {
      public: VRouterI18n.t("无需鉴权"),
      inference: VRouterI18n.t("内网匿名 / Key / OIDC 归因"),
      manage: VRouterI18n.t("需 VOS 用户或网关 Key"),
    };
    VRouterI18n.bind(container, "innerHTML", () => (doc.groups || [])
      .map(
        (g) => `<section class="card">${g.id === "resource" ? `<details><summary><strong>${esc(g.title)}</strong> · ${g.endpoints.length} ${VRouterI18n.t("项操作")}</summary>` : `<h2>${esc(g.title)}</h2>`}` +
          g.endpoints
            .map(
              (e) => `
        <details class="ep">
          <summary class="ep-head">
            <span class="pill method m-${esc(e.method.toLowerCase())}">${esc(e.method)}</span>
            <span class="mono ep-path">${esc(e.path)}</span>
            <span class="muted">${esc(e.summary)}</span>
            <span class="auth-tag">${esc(authLabel[e.auth] || e.auth)}</span>
          </summary>
          <div class="ep-desc">${esc(e.desc)}</div>
          ${e.example ? `<pre class="ep-example" data-copy="${esc(e.example)}">${esc(e.example)}</pre>
            <button class="btn" onclick="app.copyExample(this)">${VRouterI18n.t("复制示例")}</button>` : ""}
        </details>`,
            )
            .join("") +
          `${g.id === "resource" ? "</details>" : ""}</section>`,
      )
      .join(""));
  },

  async downloadOpenAPI() {
    try {
      const lang = VRouterI18n.locale === "en" ? "en" : "zh-CN";
      const doc = await this.api("GET", `/api/v1/openapi.json?lang=${lang}`);
      const url = URL.createObjectURL(new Blob([JSON.stringify(doc,null,2)],{type:"application/json"}));
      const a=document.createElement("a"); a.href=url; a.download="v-router-openapi.json"; a.click(); setTimeout(()=>URL.revokeObjectURL(url),1000);
    } catch(err) {this.toast(err.message);}
  },

  async copyExample(btn) {
    const text = btn.previousElementSibling?.dataset.copy || "";
    this.copyText(text);
  },

  // ---------- playground ----------
  pgAppend(role, text) {
    const empty = root.querySelector(".chat-empty");
    if (empty) empty.remove();
    const el = document.createElement("div");
    el.className = "msg " + role;
    el.textContent = text;
    $("pgChat").appendChild(el);
    $("pgChat").scrollTop = $("pgChat").scrollHeight;
    return el;
  },

  pgStop() {
    if (!this.pgRequest || this.pgRequest.signal.aborted) return;
    const requestId = this.pgQueueId;
    this.pgRequest.abort();
    if(requestId) void this.api('DELETE','/api/v1/inference-queue/'+encodeURIComponent(requestId)).catch(()=>{});
    $("pgSend").disabled = true;
  },
  async pgSend() {
    if (this.pgRequest) { this.pgStop(); return; }
    const input = $("pgInput");
    const text = input.value.trim();
    const model = $("pgModel").value;
    const endpoint = this.playgroundEndpoint();
    const pinned = $("pgUpstream").value;
    const native = this.state.models.find(m => m.qualified === model)?.id || model;
    if (!text || !model || !endpoint || !pinned || $("pgSend").disabled) return;
    input.value = "";
    this.pgAppend("user", text);
    const out = this.pgAppend("assistant", "…");
    const controller = new AbortController();
    this.pgRequest = controller;
    const queueId = crypto.randomUUID();
    this.pgQueueId = queueId;
    VRouterI18n.bind($("pgSend"), "textContent", () => VRouterI18n.t("停止"));
    const stream = endpoint.stream && $("pgStream").checked;
    const started = performance.now();
    let firstToken = null;
    let phase = "等待排队或服务响应";
    let queuePoll = false;
    let headersReceived = false;
    let queuePosition = null;
    let reasoningLength = 0;
    let answer = "";
    const showProgress = () => {
      const seconds = ((performance.now() - started) / 1000).toFixed(1);
      const status = `${VRouterI18n.t(phase)}${queuePosition===null?"":` · #${queuePosition}`} · ${seconds}s` + (reasoningLength ? ` · ${reasoningLength} ${VRouterI18n.t("思考字符")}` : "");
      $("pgMeta").textContent = status;
      if (!answer) out.textContent = status;
    };
    showProgress();
    const progressTimer = setInterval(() => {
      showProgress();
      if(queuePoll||controller.signal.aborted||headersReceived)return;
      queuePoll=true;
      void this.api('GET','/api/v1/inference-queue').then(data=>{
        if(controller.signal.aborted||headersReceived)return;
        const entry=data.items.find(item=>item.request_id===queueId);
        queuePosition=entry?.position??null;
        if(entry)phase="等待计算资源";
      }).catch(()=>{}).finally(()=>{queuePoll=false});
    }, 1000);
    try {
      const headers = await this.authHeaders();
      headers["Content-Type"] = "application/json";
      headers["X-VRouter-Upstream"] = pinned;
      headers["X-VScheduler-Queue-Wait-Seconds"] = "120";
      headers["X-VScheduler-Priority"] = $("pgPriority").value;
      headers["X-VScheduler-Preemptible"] = String($("pgPreemptible").checked);
      headers["X-VScheduler-Allow-Preemption"] = String($("pgAllowPreemption").checked);
      headers["X-VScheduler-Force-Admission"] = String($("pgForceAdmission").checked);
      headers["X-VScheduler-Request-ID"] = queueId;
      const body = this.playgroundRequestBody(endpoint.id, native, text, stream);
      const res = await fetch(API + "/v1" + endpoint.path, {
        method: "POST",
        headers,
        body: JSON.stringify(body),
        signal: controller.signal,
      });
      if (!res.ok) {
        let msg = `HTTP ${res.status}`;
        try {
          const raw = (await res.text()).trim().slice(0, 2000);
          let detail = raw;
          try { const data = JSON.parse(raw); detail = apiError(data,res.status).message; } catch {}
          if (typeof detail === "string" && detail) msg += `: ${detail}`;
        } catch {}
        out.remove();
        this.pgAppend("error", msg);
        return;
      }
      headersReceived=true;
      queuePosition=null;
      phase = "等待模型输出";
      showProgress();
      if (!stream) {
        const data = await res.json();
        answer = this.playgroundText(endpoint.id, data);
        out.textContent = answer;
      } else {
        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buf = "";
        const consumeLine = line => {
          line = line.trim();
          if (!line.startsWith("data:")) return;
          const payload = line.slice(5).trim();
          if (!payload || payload === "[DONE]") return;
          let data;
          try { data = JSON.parse(payload); } catch { throw new Error(VRouterI18n.t("流式响应包含无效数据")); }
          const event = playgroundStreamEvent(endpoint.id, data);
          if (event.reasoning) {
            reasoningLength += event.reasoning.length;
            phase = "模型正在思考";
          }
          if (event.text) {
            if (firstToken === null) firstToken = performance.now() - started;
            phase = "正在接收回复";
            answer += event.text;
            out.textContent = answer;
            $("pgChat").scrollTop = $("pgChat").scrollHeight;
          }
          showProgress();
        };
        for (;;) {
          const { done, value } = await reader.read();
          if (done) {
            buf += decoder.decode();
            if (buf.trim()) consumeLine(buf);
            break;
          }
          buf += decoder.decode(value, { stream: true });
          let idx;
          while ((idx = buf.indexOf("\n")) >= 0) {
            const line = buf.slice(0, idx);
            buf = buf.slice(idx + 1);
            consumeLine(line);
          }
        }
      }
      if (!answer) throw new Error(VRouterI18n.t(reasoningLength ? "模型仅返回思考内容，未返回正文" : "模型未返回正文"));
      const total = performance.now() - started;
      VRouterI18n.bind($("pgMeta"), "textContent", () => `${endpoint.label} · ${model} · ${VRouterI18n.t("上游")} ${res.headers.get("X-VRouter-Upstream") || pinned || VRouterI18n.t("未知")}（${VRouterI18n.t("指定")}） · ${VRouterI18n.t("总耗时")} ${(total / 1000).toFixed(2)}s` +
        (firstToken !== null ? ` · ${VRouterI18n.t("首")} token ${(firstToken / 1000).toFixed(2)}s` : ""));
    } catch (err) {
      if (controller.signal.aborted) {
        out.textContent = answer || VRouterI18n.t("已停止生成");
        VRouterI18n.bind($("pgMeta"), "textContent", () => VRouterI18n.t("已停止生成"));
      } else {
        out.remove();
        this.pgAppend("error", String(err));
      }
    } finally {
      clearInterval(progressTimer);
      this.pgRequest = null;
      $("pgSend").disabled = false;
      VRouterI18n.bind($("pgSend"), "textContent", () => VRouterI18n.t("发送"));
    }
  },
  playgroundRequestBody(endpoint, model, text, stream) {
    if (endpoint === "responses") return { model, stream, input: text, store: false };
    if (endpoint === "messages") return { model, stream, max_tokens: 1024, messages: [{ role: "user", content: text }] };
    if (endpoint === "completions") return { model, stream, prompt: text, max_tokens: 1024 };
    if (endpoint === "embeddings") return { model, input: text };
    if (endpoint === "videos") return { model, prompt: text };
    return { model, stream, messages: [{ role: "user", content: text }] };
  },
  playgroundText(endpoint, data) {
    if (endpoint === "responses") return data.output_text || data.output?.flatMap(o => o.content || []).map(c => c.text || "").join("") || JSON.stringify(data);
    if (endpoint === "messages") return data.content?.map(c => c.text || "").join("") || JSON.stringify(data);
    if (endpoint === "completions") return data.choices?.[0]?.text ?? JSON.stringify(data);
    if (endpoint === "embeddings") return `${VRouterI18n.t("向量")} ${data.data?.length || 0} ${VRouterI18n.t("条")} · ${VRouterI18n.t("维度")} ${data.data?.[0]?.embedding?.length || 0}`;
    if (endpoint === "videos") return JSON.stringify(data, null, 2);
    return data.choices?.[0]?.message?.content ?? JSON.stringify(data);
  },
};

// ---------- helpers ----------
function chipWithCopy(label, url) {
  if (!url) return "";
  return `<span class="endpoint-chip">${esc(label)} · ${esc(url)}<button class="copy" title="${VRouterI18n.t("复制")}">${COPY_SVG}</button></span>`;
}
function stat(icon, num, label) {
  return `<div class="stat"><div class="stat-icon">${icon}</div><div><div class="num">${num}</div><div class="label">${label}</div></div></div>`;
}
function kindLabel(kind) {
  return { registered: VRouterI18n.t("注册后端"), local: VRouterI18n.t("本地"), manual: VRouterI18n.t("手动"), cloud: VRouterI18n.t("云端") }[kind] || kind;
}
function splitList(value) {
  return String(value ?? "").split(/[\n,， ]+/).map((s) => s.trim()).filter(Boolean);
}
function modelKindOptions(kind) {
  return [
    ["", VRouterI18n.t("自动（按接口/能力推断）")],
    ["chat", VRouterI18n.t("对话")],
    ["embedding", "Embedding"],
    ["unknown", VRouterI18n.t("由上游决定")],
  ].map(([value, label]) => `<option value="${value}" ${kind === value ? "selected" : ""}>${esc(label)}</option>`).join("");
}
/** table(headers, rows, rightAlignCols) — rows contain pre-escaped HTML. */
function table(headers, rows, rightAlign = []) {
  const head = headers.map((h, i) => `<th class="${rightAlign[i] ? "num" : ""}">${h}</th>`).join("");
  const body = rows.map((r) => `<tr>${r.map((c, i) => `<td class="${rightAlign[i] ? "num" : ""}">${c}</td>`).join("")}</tr>`).join("");
  return `<div class="table-wrap"><table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table></div>`;
}
const iconServer = () => '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="3" y="4" width="18" height="7" rx="1.5"/><rect x="3" y="13" width="18" height="7" rx="1.5"/><path d="M7 7.5h.01M7 16.5h.01"/></svg>';
const iconLayers = () => '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2 3 7l9 5 9-5-9-5z"/><path d="M3 12l9 5 9-5"/><path d="M3 17l9 5 9-5"/></svg>';
const iconActivity = () => '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 12h-4l-3 9L9 3l-3 9H2"/></svg>';
const iconTimer = () => '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="13" r="8"/><path d="M12 9v4l2 2M9 2h6"/></svg>';

const localeChanged = () => {
  // Rerender chrome, keeping the chat transcript and edited form values intact.
  const ids = ["pgModel", "pgEndpoint", "pgUpstream", "pgPriority"];
  const selected = ids.map(id => $(id).value);
  const streaming = $("pgStream").checked;
  app.renderAll(true);
  ids.forEach((id, index) => { if ([...$(id).options].some(option => option.value === selected[index])) $(id).value = selected[index]; });
  $("pgStream").checked = streaming;
  app.updatePlaygroundMode();
  if ($("aboutDialog").open) app.openAbout();
  if ($("upstreamDialog").open) app.updateUpstreamProtocol();
  if (app.state.tab === "apidoc") app.renderApiDoc();
};
window.app = app;
const started = app.start();
return { started, updatePrivileges: () => app.renderUser(app.state.status), select: tab => app.switchTab(tab), updateLocale: () => { i18n.apply(); localeChanged(); }, dispose: () => { app.pgStop(); clearInterval(app.state.timer); if(window.app===app)delete window.app; } };

}
