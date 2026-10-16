> 迁移说明：网关现由 V-Scheduler 统一发布，Rust 源码位于 `backend/src/router/`；文中 Go 测试命令仅为历史记录。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# 协议转换性能验证

日期：2026-09-10。测试环境：Linux amd64、AMD EPYC 7452；HTTP 压测限制 `GOMAXPROCS=4`。上游是同进程、同机 loopback 的 Chat Completions fixture，不包含模型推理、公网延迟、TLS 或真实 VOS 入口代理。

每组预热 64 次、采样 1,000 次；流式上游输出 8 个文本分片。基线是同一份源码的原生 Chat Completions 代理，共享连接池设置一致。延迟是完整 HTTP 往返；首文本延迟统计实际文本到达，不把 response.created 当成首 token。不同协议的输出字节数不同，因此吞吐差异也包含协议编码成本。

验收：测试组合中转换相对基线的 P95 追加延迟不超过 10 ms；正确性、取消和资源上限同时通过。此处是测试环境的验收结果，不是生产延迟 SLA，也不推断跨架构性能。

| 输入 | 流式 | 并发 | 协议 | 请求/秒 | P50 ms | P95 ms | 首文本 P95 ms | 响应字节 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 KiB | 否 | 1 | chat/completions | 1517 | 0.659 | 0.832 | 0.000 | 181 |
| 1 KiB | 否 | 1 | responses | 1412 | 0.688 | 0.850 | 0.000 | 575 |
| 1 KiB | 否 | 1 | messages | 1404 | 0.692 | 0.822 | 0.000 | 285 |
| 1 KiB | 否 | 32 | chat/completions | 4643 | 5.513 | 11.708 | 0.000 | 181 |
| 1 KiB | 否 | 32 | responses | 7883 | 4.144 | 6.976 | 0.000 | 575 |
| 1 KiB | 否 | 32 | messages | 7709 | 4.008 | 7.491 | 0.000 | 285 |
| 1 KiB | 是 | 1 | chat/completions | 1266 | 0.760 | 1.135 | 1.043 | 549 |
| 1 KiB | 是 | 1 | responses | 778 | 1.208 | 1.830 | 1.290 | 4385 |
| 1 KiB | 是 | 1 | messages | 828 | 1.129 | 1.701 | 1.263 | 1688 |
| 1 KiB | 是 | 32 | chat/completions | 5532 | 5.412 | 10.436 | 10.348 | 549 |
| 1 KiB | 是 | 32 | responses | 3029 | 10.030 | 18.795 | 17.584 | 4385 |
| 1 KiB | 是 | 32 | messages | 3608 | 8.240 | 15.445 | 15.250 | 1688 |
| 64 KiB | 否 | 1 | chat/completions | 204 | 4.869 | 5.812 | 0.000 | 181 |
| 64 KiB | 否 | 1 | responses | 251 | 3.929 | 5.049 | 0.000 | 575 |
| 64 KiB | 否 | 1 | messages | 232 | 4.272 | 5.390 | 0.000 | 285 |
| 64 KiB | 否 | 32 | chat/completions | 981 | 30.424 | 56.207 | 0.000 | 181 |
| 64 KiB | 否 | 32 | responses | 1398 | 20.592 | 45.920 | 0.000 | 575 |
| 64 KiB | 否 | 32 | messages | 1311 | 21.980 | 50.223 | 0.000 | 285 |
| 64 KiB | 是 | 1 | chat/completions | 202 | 4.869 | 5.894 | 5.823 | 549 |
| 64 KiB | 是 | 1 | responses | 223 | 4.391 | 5.778 | 5.278 | 4385 |
| 64 KiB | 是 | 1 | messages | 213 | 4.619 | 5.802 | 5.377 | 1688 |
| 64 KiB | 是 | 32 | chat/completions | 954 | 31.928 | 57.438 | 57.437 | 549 |
| 64 KiB | 是 | 32 | responses | 1080 | 27.528 | 53.772 | 50.003 | 4385 |
| 64 KiB | 是 | 32 | messages | 1040 | 29.134 | 53.427 | 53.426 | 1688 |

1 KiB / 32 并发时，Responses 流式 P95 相对基线增加 8.359 ms，Messages 增加 5.009 ms；这也是本次组合中的最大追加值所在场景。更长输入的转换路径省去了重复模型重写，部分结果快于直通路径，但短时 loopback 测试不能据此承诺普遍加速。

请求转换微基准（同机，`GOMAXPROCS=4`）：1 KiB 为 17.394 µs/op、2,600 B/op、22 次分配；64 KiB 为 662.724 µs/op、67,286 B/op、22 次分配。此项仅包含请求解析和映射，不包含 HTTP 或响应编码。官方 SDK 严格校验使用 OpenAI Python 2.54.0、Anthropic Python 1.4.0，均通过文本、流式、工具调用和用量验证。

实现约束：Go 进程内转换，无额外运行时/HTTP 跳转；连接池每上游最多保留 64 个空闲连接；原生代理和转换共享池。SSE 按上游事件增量处理，每批事件立即 flush；文本和工具参数使用 builder 累积终态数据，上限 32 MiB。客户端取消传播至上游，不自动重试生成。没有用固定休眠或等待完整回答来换取吞吐。

复现时先把当前本地源码 rsync 到架构匹配的构建机，再在该副本运行（所有 Git 操作留在本地）：

```bash
cd ~/dev/repos/V-Router/gateway
V_ROUTER_PERF=1 GOMAXPROCS=4 go test -run '^TestProtocolPerformance$' -count=1 -timeout 120s -v ./internal/api
GOMAXPROCS=4 go test -run '^$' -bench BenchmarkConvertRequest -benchmem ./internal/api
```

性能测试不加 `-race`；正确性另跑 `go test -race -timeout 60s ./...`。高并发长上下文、慢消费者、不同 CPU 和真实入口代理需要在部署环境复测，不使用此结果估算模型推理容量。
