<script setup lang="ts">
import {ref,onMounted,computed} from 'vue'
import {request,base} from './client'
defineProps<{t:(text:string)=>string}>()
const integration=ref<any>(null),spec=ref<any>(null),error=ref(''),filter=ref(''),operationKey=ref('POST /api/v1/models/pull'),externalHost=ref(location.hostname)
const operations=computed(()=>Object.entries(spec.value?.paths||{}).flatMap(([path,methods])=>Object.entries(methods as any).map(([method,op])=>({path,method,key:method.toUpperCase()+' '+path,...op as any}))))
const visible=computed(()=>operations.value.filter(op=>`${op.path} ${op.summary}`.toLowerCase().includes(filter.value.toLowerCase())))
const operation=computed(()=>operations.value.find(op=>op.key===operationKey.value)||operations.value[0])
function expand(value:any,depth=0):any {if(depth>12||!value||typeof value!=='object')return value;if(value.$ref){const target=value.$ref.split('/').slice(1).reduce((obj:any,key:string)=>obj?.[key],spec.value);return expand(target,depth+1)}if(Array.isArray(value))return value.map(item=>expand(item,depth+1));return Object.fromEntries(Object.entries(value).map(([key,item])=>[key,expand(item,depth+1)]))}
const curl=computed(()=>{const op=operation.value;if(!op)return '';const example=op.requestBody?.content?.['application/json']?.example;const query=(op.parameters||[]).filter((p:any)=>p.in==='query').map((p:any)=>`${p.name}=${p.name==='source'?'hf':'hf%3A%2F%2Forg%2Frepo'}`).join('&');return `curl -fsS -X ${op.method.toUpperCase()} 'http://${externalHost.value||'<vos-host>'}:${integration.value?.endpoints?.find((e:any)=>e.name==='Model management')?.host_port||36005}${op.path.replace('{id}','<task-or-service-id>')}${query?'?'+query:''}'`+(example?` \\\n  -H 'Content-Type: application/json' \\\n  -d '${JSON.stringify(example)}'`:op.path.endsWith('import-archive')?` \\\n  -F source=hf -F model_id=local/my-model -F archive=@model.tar.gz`:'')})
const downloadExample=`BASE=http://v-scheduler-model-hub:6005
# External caller: BASE=http://<vos-host>:36005
curl -fsS --get "$BASE/api/v1/models/repository-files" \\
  --data-urlencode source=hf --data-urlencode model_id=org/repo
curl -fsS -X POST "$BASE/api/v1/models/pull" \\
  -H 'Content-Type: application/json' \\
  -d '{"source":"hf","model_id":"hf://org/repo","files":["config.json","weights/model-Q4_K_M.gguf"]}'
# Save the returned task_id; poll until READY or FAILED.
curl -fsS "$BASE/api/v1/tasks/detail/<task_id>"
curl -fsS --get "$BASE/api/v1/models/files" \\
  --data-urlencode model_id=hf://org/repo
# Add another quantization while retaining existing files:
curl -fsS -X POST "$BASE/api/v1/models/pull" \\
  -H 'Content-Type: application/json' \\
  -d '{"source":"hf","model_id":"hf://org/repo","files":["weights/model-Q8_0.gguf"],"append":true}'
curl -fsS -X POST "$BASE/api/v1/tasks/<task_id>/cancel"
curl -fsS -X POST "$BASE/api/v1/tasks/<task_id>/retry"`
const pythonExample=`import json, time
from urllib.request import Request, urlopen
BASE = "http://v-scheduler-model-hub:6005"
def request(path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = Request(BASE + path, data=data,
                  headers={"Content-Type": "application/json"})
    with urlopen(req, timeout=60) as response:
        return json.load(response)
created = request("/api/v1/models/pull", {
    "source": "hf", "model_id": "hf://org/repo",
    "files": ["config.json", "weights/model-Q4_K_M.gguf"]})
deadline = time.monotonic() + 7200
while time.monotonic() < deadline:
    task = request("/api/v1/tasks/detail/" + created["task_id"])
    if task["phase"] == "FAILED":
        raise RuntimeError(task.get("error_msg"))
    if task["phase"] == "READY":
        break
    time.sleep(2)
else:
    raise TimeoutError("Client wait timed out; server task continues")
print(request("/api/v1/models/files?model_id=hf%3A%2F%2Forg%2Frepo"))`
const layout=`<shared directory>/
├── ollama/models/{blobs,manifests}/
├── export/hf/<org>/<repo>/snapshots/<revision>/
├── export/hf/<org>/<repo>/current -> snapshots/<revision>
├── export/ms/<org>/<repo>/current -> snapshots/<revision>
├── registry/{models,aliases,containers,inference_services}.json
└── shared/{tasks,tmp,locks}/

# Other VOS apps mount the SAME installer-selected path read-only:
volumes:
  - \${MODEL_HUB_SHARED_MODELS_PATH}:/modelhub:ro
# Example inference model path:
/modelhub/export/hf/org/repo/current/weights/model-Q4_K_M.gguf`
const registration=`curl -fsS -X POST http://v-scheduler-model-hub:6005/api/v1/inference-services/register \\
 -H 'Content-Type: application/json' \\
 -d '{"service_id":"vllm-qwen","name":"vLLM Qwen","kind":"openai_compatible","base_url":"http://vllm:8000","gateway_url":"http://vllm:8000","role":"generate","api_contract":{"health_path":"/health","models_path":"/v1/models","running_models_path":"/v1/models","metrics_path":"/management/metrics","start_model_path":"/management/start","stop_model_path":"/management/stop","model_action_status_path":"/management/action-status"}}'
# Custom paths above are an example contract; provide paths implemented by your backend.
# Start/stop POST: {model, mode?, keep_alive?}. Status GET appends model/action.
# Plain vLLM does not supply model lifecycle endpoints by default.`
onMounted(async()=>{try{[integration.value,spec.value]=await Promise.all([request('/api/v1/integration'),request('/openapi.json')])}catch(e){error.value=e instanceof Error?e.message:String(e)}})
</script>
<template>
 <article class="hub-guide panel">
  <h2>{{t('模型管理接入指南 / API')}}</h2><p v-if="error" class="error">{{error}}</p>
  <nav><a href="#hub-endpoints">{{t('调用入口与认证边界')}}</a><a href="#hub-download">{{t('选择下载与追加文件')}}</a><a href="#hub-storage">{{t('模型路径与应用挂载')}}</a><a href="#hub-runtime">{{t('推理服务')}}</a><a href="#hub-api">{{t('API 文档')}}</a></nav>
  <h3 id="hub-endpoints">{{t('调用入口与认证边界')}}</h3>
  <label>{{t('VOS 宿主机地址')}} <input v-model="externalHost" placeholder="<vos-host>"></label>
  <p>{{t('Docker DNS 地址用于同一 vos_default 网络内的应用；外部调用使用宿主机地址与映射端口。')}}</p>
  <div class="table-wrap"><table><thead><tr><th>{{t('用途')}}</th><th>Docker host alias : port</th><th>{{t('宿主机地址 : 映射端口')}}</th><th>{{t('认证')}}</th></tr></thead><tbody><tr v-for="endpoint in integration?.endpoints||[]" :key="endpoint.name"><td>{{endpoint.name}}</td><td><code>{{endpoint.internal}}{{endpoint.path}}</code></td><td><code>http://{{externalHost||'&lt;vos-host&gt;'}}:{{endpoint.host_port}}{{endpoint.path}}</code></td><td>{{endpoint.authentication}}</td></tr></tbody></table></div>
  <p>{{t('模型管理 API 保持 Model Hub 原有匿名调用契约；VOS HTTPS 不等同于 API 身份认证。管理端口应限定在受控网络。调度 API 与统一推理 API 继续使用各自的身份校验。')}}</p>
  <p>{{t('VOS 页面内的模型管理入口：')}}<code>/app/com.ictrek.v-scheduler/model-hub/api/v1</code> · <a :href="base+'/openapi.json'" target="_blank" rel="noopener">OpenAPI JSON</a></p>
  <h3 id="hub-download">{{t('选择下载与追加文件')}}</h3><p>{{t('先查询仓库文件列表，再传入精确相对路径。省略 files 下载整仓；append=true 补下载已有模型，保留旧文件。选择范围不支持通配符、绝对路径或上级目录。')}}</p><pre>{{downloadExample}}</pre>
  <table><thead><tr><th>{{t('字段')}}</th><th>{{t('类型')}}</th><th>{{t('含义与约束')}}</th></tr></thead><tbody><tr v-for="row in [['model_id','string','ollama://name:tag / hf://org/repo / ms://org/repo'],['source','enum','ollama / hf / ms'],['files','string[] | null',t('精确文件列表；null 表示整仓或历史未知范围')],['append','boolean',t('仅用于 HF/MS 已有模型，必须提供非空 files')],['force','boolean',t('允许更新已有 READY 模型')],['progress','number','0 .. 1'],['speed / eta','number | null','bytes/s · seconds'],['phase','enum','PENDING / DOWNLOADING / VERIFYING / READY / FAILED'],['error_msg','string | null',t('失败或取消原因')]]" :key="row[0]"><td><code>{{row[0]}}</code></td><td>{{row[1]}}</td><td>{{row[2]}}</td></tr></tbody></table>
  <p>{{t('READY 仅表示所选范围完成，不表示整仓齐全或推理所需依赖齐备。下载失败与取消保留旧模型；重试保留原选择范围并复用可续传数据。删除任务记录不会删除模型。')}}</p><details><summary>{{t('调用方 Python 完整示例：创建任务并等待完成')}}</summary><pre>{{pythonExample}}</pre></details>
  <h3 id="hub-storage">{{t('模型路径与应用挂载')}}</h3><p>{{t('安装时选择原 Model Hub 的共享目录即可接管，无需重新下载。保留目录中的注册表和文件；原应用与新版可共存，请避免同时下载、更新或删除同一模型。')}}</p><p><code>{{integration?.shared_path||'/data/vos_workspace/model_hub'}}</code> → <code>/data</code> · hint <code>[com.ictrek.model.storage]</code></p><pre>{{layout}}</pre>
  <p>{{t('外部下载工具写入 snapshots 目录，下载期间保留 .incomplete / .tmp 标记；完成后才切换 current。应用每 60 秒扫描一次，也可调用 models/sync。不要把未完成版本暴露为 current。旧 HF/MS 缓存会原地识别，原文件不搬迁。')}}</p>
  <p>{{t('试聊统一使用 V-Scheduler 已有试聊页面。Router 直接接入三个内置 Ollama，上游命名 v-scheduler/ollama-qa、v-scheduler/ollama-embedding、v-scheduler/ollama-rerank，无需通过模型管理发现。')}}</p>
  <p>{{t('三个内置 Ollama 后端自动纳入调度，可直接试聊，无需逐个批准模型。资源忙时统一调用等待计算资源。')}}</p>
  <h3 id="hub-runtime">{{t('推理服务')}}</h3><p>{{t('三个内置 Ollama 实例分别处理聊天、向量和重排，共享模型文件，各自配置并行数和上下文。安装默认关闭三个预热开关。通过卡片查看已下载/运行模型、内存、槽位、prefill/decode 和速度，启动或停止模型。')}}</p><p>{{t('Ollama 原生管理 API 使用 11434；带指标的推理网关使用 11535。外部客户端使用表中映射后的端口。统一模型调用优先走 V-Router，保留调度准入。')}}</p><pre>POST http://v-scheduler-ollama-qa:11535/v1/chat/completions
POST http://v-scheduler-ollama-embedding:11535/v1/embeddings
POST http://v-scheduler-ollama-rerank:11535/v1/rerank</pre><p>{{t('外部 Ollama、OpenAI 兼容或自定义推理服务通过注册接口接入。非 Ollama 服务必须提供实际实现的管理路径；操作返回 accepted 与 confirmed，只有状态确认后才显示完成。')}}</p><pre>{{registration}}</pre>
  <h3>{{t('错误处理与网络配置')}}</h3><p>{{t('400 表示请求无效，404 表示记录不存在，409 表示状态冲突或上游/存储不可用。取消另一个应用拥有的下载应在原应用完成。HF 镜像开关默认关闭；开启后使用安装参数 HF_ENDPOINT。私有仓库令牌由服务环境提供，不能放入 URL 或 API 文档。')}}</p>
  <h3 id="hub-api">{{t('API 文档')}}</h3><input v-model="filter" :placeholder="t('搜索接口')"><div class="hub-api-layout"><div><button v-for="op in visible" :key="op.key" :class="{current:op.key===operationKey}" @click="operationKey=op.key"><b>{{op.method.toUpperCase()}}</b> {{op.path}}</button></div><section v-if="operation"><h4>{{operation.summary}}</h4><p><code>{{operation.key}}</code></p><pre>{{curl}}</pre><h4>{{t('参数')}}</h4><pre>{{JSON.stringify(operation.parameters,null,2)}}</pre><h4>{{t('请求')}}</h4><pre>{{JSON.stringify(expand(operation.requestBody||{}),null,2)}}</pre><h4>{{t('响应')}}</h4><pre>{{JSON.stringify(expand(operation.responses),null,2)}}</pre></section></div>
 </article>
</template>
<style scoped>
.hub-guide{padding:24px;line-height:1.8;min-width:0}.hub-guide nav{display:flex;gap:16px;flex-wrap:wrap}.hub-guide h3{margin-top:30px;scroll-margin:20px}.hub-guide pre{white-space:pre-wrap;overflow-wrap:anywhere;padding:18px;background:#f4f7fb;border:1px solid #e1e8f2;border-radius:10px;font-size:12px;line-height:1.7}.hub-guide code{overflow-wrap:anywhere;font-size:12px}.table-wrap{overflow:auto}.hub-guide table{width:100%;margin:18px 0}.hub-guide th,.hub-guide td{padding:12px;text-align:left;border-bottom:1px solid #e4ebf4}.hub-guide a{color:#2563bb}.hub-guide input{padding:9px;border:1px solid #d5dfeb;border-radius:8px}.hub-api-layout{display:grid;grid-template-columns:minmax(230px,1fr) minmax(0,2fr);gap:20px;margin-top:16px}.hub-api-layout>div{max-height:680px;overflow:auto}.hub-api-layout button{display:block;width:100%;text-align:left;overflow-wrap:anywhere;font-size:11px;margin-bottom:5px}.hub-api-layout button.current{background:#e8f1ff;color:#1a59b4}.hub-api-layout b{display:inline-block;min-width:48px}@media(max-width:800px){.hub-guide{padding:14px}.hub-api-layout{grid-template-columns:1fr}.hub-api-layout>div{max-height:240px}}
</style>
