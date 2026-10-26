<script setup lang="ts">
import {ref,onMounted} from 'vue'
import {request} from './modelhub/client'
const endpoints=ref<any[]>([])
const locationHost=location.hostname||'<vos-host>'
onMounted(async()=>{try{endpoints.value=(await request('/api/v1/integration')).endpoints}catch{}})
defineProps<{t:(value:string)=>string}>()
</script>
<template>
 <article class="integration-guide panel">
  <h2>{{t('应用接入')}}</h2>
  <p>{{t('模型客户端继续使用网关 API；调度管理使用独立前缀。')}}</p>
  <p>{{t('统一容器的正式 host alias 只有 v-scheduler：8080 为网关，38120 为调度，6005 为模型管理。表中其余名称均为兼容别名，指向同一容器；三个 Ollama 是独立容器。新应用和已有应用后续适配均推荐使用 v-scheduler。')}}</p>
  <div class="endpoint-table"><table><thead><tr><th>{{t('用途')}}</th><th>{{t('正式 host alias : 内部端口')}}</th><th>{{t('兼容别名')}}</th><th>{{t('宿主机地址 : 映射端口')}}</th></tr></thead><tbody><tr v-for="endpoint in endpoints" :key="endpoint.name"><td>{{endpoint.name}}</td><td><code>{{endpoint.internal}}{{endpoint.path}}</code></td><td><div v-for="alias in endpoint.aliases||[]" :key="alias"><code>{{alias}}</code></div><div v-for="alias in endpoint.compatibility_aliases||[]" :key="alias">{{t('兼容')}}：<code>{{alias}}</code></div><span v-if="!endpoint.aliases?.length&&!endpoint.compatibility_aliases?.length">—</span></td><td><code>http://{{locationHost}}:{{endpoint.host_port}}{{endpoint.path}}</code></td></tr></tbody></table></div>
  <p>{{t('端口显示当前安装配置。模型管理 API、选定权重文件下载及三个 Ollama 接口的详细说明见「模型管理 API」。')}}</p>
  <p>{{t('正式别名由当前 V-Scheduler 安装声明；兼容别名仅供旧客户端使用。新应用优先使用正式别名；与旧 V-Router 共存时不要向兼容别名发送私有凭据。所有别名均指向现有容器，不新增服务。')}}</p>
  <h3>{{t('本地与云端模型')}}</h3>
  <p>{{t('本地和纳管远端模型先取得资源许可；云端供应商模型不占本地资源预约。资源忙时遵守重试提示，不能绕过网关直接启动同一次计算。')}}</p>
  <h3>{{t('Provider 与模型能力')}}</h3>
  <p>{{t('上游页面可选厂商预设或自定义中转地址，测试后勾选要导入的模型。模型能力和接口使用复选框；普通聊天上游缺失声明时默认 chat / completion 与 chat_completions，Anthropic 默认 messages。显式接口优先，图片、语音、向量和决策模型不会自动声明为聊天；tools 和 vision 必须按真实能力勾选。')}}</p>
  <p>{{t('Ollama 模型通过 /api/show 自动读取 completion、vision、tools、thinking、embedding、decision 等实际能力，内置实例与手动添加的实例使用同一逻辑。旧版 OpenAI 兼容配置通过 /api/version 识别；读取失败保留已有信息。手动修改后刷新不会覆盖，点击「重新识别上游能力」可恢复自动识别。')}}</p>
  <h3>{{t('Jev 原生决策与中转')}}</h3>
  <p>{{t('TypeSafe / Jev 使用 typesafe 协议和 https://api.typesafe.ai/v1，也可使用实现同协议的自有中转。模型列表读取 models[].name；能力为 decision，接口为 systemone，不会冒充 Harness 聊天模型。工作台选择 System One 后输入包含 state、questions 的 JSON 测试。')}}</p>
  <p><code>POST http://v-scheduler:8080/v1/systemone</code> · <code>http://{{locationHost}}:22888/v1/systemone</code></p>
  <p>{{t('局域网端口以安装配置为准。同域入口为 /api/com.ictrek.v-scheduler/v1/systemone；鉴权沿用本人 VOS token 或网关 Key，受限应用凭据保留原白名单。供应商密钥保存在网关，请求仅重写模型 ID，保留 state、questions、assets、功能头及原生 answers、概率、置信度和 usage；不支持 SSE。')}}</p>
  <h3>{{t('应用和模型自动发现')}}</h3>
  <p>{{t('安装支持的应用后，网关会按发现间隔读取其可用模型与服务。内置三个 Ollama 由本应用直接接入；其他应用支持 Image Studio、V-Voice、V-Music、ML Server 和视频结构化服务。')}}</p>
  <p>{{t('V-Voice 就绪后会主动接入调度器，自动绑定已校准的执行器与模型映射。外部云端服务可在上游页面配置。')}}</p>
  <p>{{t('V-Voice 0.0.27 保留 ONNX、声纹与 audio.cpp GGUF，移除四个 PyTorch 原生音乐模型；绑定以实际就绪模型和匹配审批为准。')}}</p>
  <h3>{{t('节点、应用、预算与停止的统一流程')}}</h3>
  <p>{{t('节点遥测由 VOS App 内置组件自动上报；应用登记已批准的执行器并定时心跳，停止请求经原生退出及权重卸载核验后释放预约。')}}</p>
  <p>{{t('节点约每 2 秒上报，超过 12 秒无心跳暂停新准入。应用按响应间隔心跳（当前 5 秒），超过 20 秒暂停该执行器新准入；心跳恢复后重新核验。用户无需 SSH 或手工部署 Agent。')}}</p>
  <p><code>GET /scheduler/api/v1/integrations</code> · <code>POST /scheduler/api/v1/integrations</code> · <code>POST /scheduler/api/v1/integrations/heartbeat</code></p>
  <pre>{"protocol_version":1,"revision":0,"executors":["approved-executor-id"],"stop_protocol":"permit-native-exit-v1"}</pre>
  <p>{{t('注册仅引用已批准的执行器，不接受自报预算或任意控制地址。应用使用私有服务身份保存 revision；同内容重注册保留版本，409 时先读取最新状态。模型路由注册与服务卡片仍按同一实例关联。')}}</p>
  <p><code>POST /scheduler/api/v1/permits/{id}/stop</code> → <code>POST /scheduler/api/v1/integrations/heartbeat</code> → <code>POST /scheduler/api/v1/permits/{id}/stopped</code></p>
  <p>{{t('停止命令包含许可、执行器、模型、execution_id 和 nonce。应用先取消实际执行并卸载权重，再携带原执行凭据回报 native_exit_barrier、model_unloaded 与 evidence；错误或缺失回执不释放资源。异步任务继续使用 tasks/{id}/cancel 与原执行报告。')}}</p>
  <p>{{t('节点管理中的「应用统一接入」可查看应用心跳、节点与批准预算，并请求停止已接入的执行。旧应用继续使用已验证的原自动绑定；不支持此停止协议的应用不会显示为已完成接管。')}}</p>
  <p>{{t('V-Voice（含配乐）、V-Sub、Desensitize 和 Image Studio 在可信绑定后自动登记。调用方 V-ChatCut、HybRAG 复用实际执行端，不重复申报预算。')}}</p>
  <p>{{t('应用私有目录保存已确认停止的回执，重启后继续重试；未知执行不推定停止。阻塞式原生调用须等待实际返回及卸载，异步任务在统一接入页面使用取消。')}}</p>
  <p>{{t('GPU 槽位是调度并发计数，不是独立显存。统一内存预算只计一份：Thor 的 SenseVoice、Kokoro 和 CAM++ 使用实测的 4 GiB 共享内存预算；较大语音与音乐模型使用各自校准。192 的 CPU 语音记录为 1.75 GiB、GPU 并发计数 0，不因统一内存就自动改为 GPU 执行。')}}</p>
  <h3>{{t('推理等待队列')}}</h3>
  <p>{{t('本地推理设置 X-VScheduler-Queue-Wait-Seconds（1..300）、X-VScheduler-Priority（background/interactive/high）和唯一 X-VScheduler-Request-ID。开启内置驻留且身份已验证时，未设置等待默认等 30 秒；显式 0 保持立即准入。试聊默认等待 120 秒，可查看位置并取消。')}}</p>
  <p><code>GET /api/v1/inference-queue</code> · <code>DELETE /api/v1/inference-queue/{id}</code></p>
  <p>{{t('队列按用户和应用隔离；每等待 30 秒提升一级优先级，默认不抢占运行中的计算；协作抢占须双方显式同意。等待不预约资源，重启不重放请求；总超时应覆盖排队、冷加载和推理。')}}</p>
  <h3>{{t('优先级与协作抢占')}}</h3>
  <p>{{t('只有接收端心跳正常且支持原生停止证明的执行器才执行抢占；其他执行继续排队等待，不强杀，不跳过停止证明。')}}</p>
  <pre>{"scheduling":{"priority":"high","preemptible":false,"allow_preemption":true,"force_admission":false}}</pre>
  <p>{{t('tasks 和 permits 接受 scheduling；默认不抢占。priority 为 background、interactive 或 high，等待每 30 秒提升一级。高优先级排队先执行；只有双方显式同意才请求低优先级运行任务让出资源。')}}</p>
  <p><code>X-VScheduler-Priority</code> · <code>X-VScheduler-Preemptible</code> · <code>X-VScheduler-Allow-Preemption</code> · <code>X-VScheduler-Force-Admission</code></p>
  <p>{{t('V-Voice、V-Sub、Desensitize 通过请求头传递这些参数；Image Studio 的生成 JSON 接受 scheduling。旧执行和权重未确认退出时继续占用预算，不能因抢占请求或超时强行释放。')}}</p>
  <p>{{t('Image Studio 的可重启任务在确认停止后回队列，使用新 attempt/fence 从头重跑，受已批准尝试次数限制。没有断点能力时不称为暂停；同步调用不会自动重放，用户取消优先于抢占重排。')}}</p>
  <p>{{t('force_admission 需要控制器再次核验管理员身份并审计；普通应用不能自行启用。通过 X-VScheduler-Admin-Authorization 临时交接当前管理员 token，不写入任务或重启回放。强制尝试仍保留实测预算、硬件容量、节点新鲜度、停止证明及至少 1 GiB 主机内存。')}}</p>
  <p>{{t('统一内存节点可由管理员明确配置 safety_reserve.memory_mib 为 1024；未配置节点保留自动余量。准入仍须满足本次实测预算、已有预约及安全保留，不能仅因剩余 1 GiB 就运行任意模型。')}}</p>
  <p>{{t('优先级属于本次请求或任务，不属于模型。已放行请求不受普通队列阻塞；其他请求按任务优先级和等待时间排序。同一后端实例的权重只保留一份，运行预算不足时排队。')}}</p>
  <h3>{{t('原生应用统一等待准入')}}</h3>
  <p>{{t('原生执行端通过 POST /scheduler/api/v1/permits/wait 申请准入，JSON 为 {"request":原 permits 请求,"wait_seconds":30}；默认等待 30 秒，X-VScheduler-Queue-Wait-Seconds 可设为 0..300，0 表示立即尝试。等待只保存身份与顺序，不预约模型权重，不启动或重放推理；超时返回 429 QUEUE_TIMEOUT。V-Voice、Desensitize、V-Sub 在本地执行锁之前进入统一队列，同一已批准原生进程只保留一条执行通道；不同进程的实际副本分别预算。授权强制请求先行，其后按本次任务优先级、等待老化和到达顺序执行。其他应用申请资源不会无条件卸载空闲 Ollama，只有内存不足才请求卸载，确认停止后才释放预算。')}}</p>
  <h3>{{t('模型驻留与卸载')}}</h3>
  <p>{{t('在模型仓库的「模型接入状态」设置空闲保留秒数，默认 60，范围 0..600；0 表示每次卸载。QA、Embedding、Rerank 可同时驻留，空闲时保留已确认内存；调用前恢复完整计算预算，资源不足才按空闲先后卸载。')}}</p>
  <p><code>GET /api/v1/model-residency</code> · <code>PUT /api/v1/model-residency</code> · <code>{"idle_seconds":60}</code></p>
  <h3>{{t('模型接入状态')}}</h3>
  <p>{{t('通过推理服务的 metadata.model_bindings 将后端模型名称关联到仓库模型 ID。状态页统一显示权重、网关路由、后端健康与调度绑定，并说明不可调用原因；配置就绪不代表当前资源一定可用。')}}</p>
  <p><code>GET /api/v1/model-lifecycle</code> · <code>{"metadata":{"model_bindings":{"served-name":"hf://org/repo"}}}</code></p>
  <p>{{t('这些接口属于网关：容器内使用 http://v-scheduler:8080，宿主机使用上表的网关映射端口。驻留设置和完整接入状态需要管理员身份。')}}</p>
  <h3>{{t('开发文档')}}</h3>
  <p><a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/application-integration.md" target="_blank" rel="noopener">{{t('应用接入指南')}}</a> · <a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/integration-standard.md" target="_blank" rel="noopener">{{t('公开 API 与接入标准')}}</a> · <a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/unified-application.md" target="_blank" rel="noopener">{{t('统一应用说明')}}</a></p>
 </article>
</template>
<style scoped>.endpoint-table{overflow:auto}.integration-guide{padding:24px;line-height:1.8}.integration-guide h3{margin-top:24px}.integration-guide table{margin:18px 0}.integration-guide a{color:#265cc2}.integration-guide code{overflow-wrap:anywhere}</style>
