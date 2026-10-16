> 迁移说明：网关现由 V-Scheduler 统一发布，源码位于 `gateway/`。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# Hosting in the unified V-Scheduler application

V-Scheduler owns the combined VOS installation package, frontend and documentation entry. This repository continues to own the gateway backend, automatic discovery and original model/management APIs. The existing Feishu column `v_router_backend` and `v-router-backend` image repository remain unchanged.

`V_ROUTER_CONTROLLER_URL=http://v-scheduler:38120` enables the combined application's `/scheduler/api/v1/*` controller proxy and unified UI at the existing gateway host port. Caller authorization and idempotency headers reach the controller unchanged; controller authentication still applies. The original `/v1/*` and gateway `/api/v1/*` handlers keep priority. Unknown unprefixed API paths do not become controller endpoints. Without this environment setting, standalone gateway routing is unchanged.

All frontend changes now belong to [V-Scheduler](https://github.com/ictrektech/V-Scheduler). The original frontend in this repository remains available for legacy packages. See the [unified application and retained-volume migration](https://github.com/ictrektech/V-Scheduler/blob/main/docs/unified-application.md) for ports, package ownership and installation steps. Do not uninstall the old gateway with data removal or run both writers against the retained directory.
