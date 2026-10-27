// Public metadata only: credentials remain in the existing server-side upstream store.
export const CAPABILITIES = [
  ['chat', '对话'], ['completion', '文本生成'], ['vision', '图片理解'],
  ['tools', '工具调用'], ['thinking', '推理'], ['embedding', 'Embedding'],
  ['image_embedding', '图片向量'], ['rerank', '重排序（Rerank）'],
  ['image_generation', '图片生成'], ['image_edit', '图片编辑'],
  ['image_variation', '图片变体'], ['video_generation', '视频生成'],
  ['asr', '语音识别'], ['tts', '语音合成'], ['translation', '翻译'],
  ['moderation', '内容审核'], ['face_recognition', '人脸识别'],
  ['person_reid', '行人重识别'], ['ocr', '文字识别'],
  ['video_structure', '视频结构化'], ['decision', '结构化决策（Jev）'],
];
export const ENDPOINTS = [
  'chat_completions', 'responses', 'messages', 'completions', 'embeddings', 'rerank',
  'images/generations', 'images/edits', 'images/variations', 'videos/generations',
  'audio/transcriptions', 'audio/translations', 'audio/speech', 'moderations',
  'ml/predictions', 'ml/video/structure', 'systemone',
].map(id => [id, id]);

// Separate region/product endpoints are intentional; aliases share one provider entry.
const LEGACY_PROVIDER_PRESETS = [
  ['openrouter', 'OpenRouter', 'https://openrouter.ai/api/v1', 'openai'],
  ['deepseek', 'DeepSeek', 'https://api.deepseek.com/v1', 'chat_completions'],
  ['bailian', 'Alibaba Cloud Bailian (China)', 'https://dashscope.aliyuncs.com/compatible-mode/v1', 'chat_completions'],
  ['bailian-intl', 'Alibaba Cloud Bailian (Intl)', 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1', 'chat_completions'],
  ['zai', 'Z.ai / GLM', 'https://api.z.ai/api/paas/v4', 'chat_completions'],
  ['bigmodel', 'BigModel / GLM', 'https://open.bigmodel.cn/api/paas/v4', 'chat_completions'],
  ['moonshot', 'Moonshot Kimi (China)', 'https://api.moonshot.cn/v1', 'chat_completions'],
  ['minimax', 'MiniMax (China)', 'https://api.minimaxi.com/v1', 'chat_completions'],
  ['mimo', 'Xiaomi MiMo', 'https://api.xiaomimimo.com/anthropic/v1', 'anthropic'],
  ['openai', 'OpenAI', 'https://api.openai.com/v1', 'openai'],
  ['anthropic', 'Anthropic', 'https://api.anthropic.com/v1', 'anthropic'],
  ['xai', 'xAI', 'https://api.x.ai/v1', 'openai'],
  ['opencode-zen', 'OpenCode Zen', 'https://opencode.ai/zen/v1', 'openai'],
  ['opencode-go', 'OpenCode Go', 'https://opencode.ai/zen/go/v1', 'openai'],
  ['gemini', 'Google Gemini', 'https://generativelanguage.googleapis.com/v1beta/openai', 'chat_completions'],
  ['mistral', 'Mistral AI', 'https://api.mistral.ai/v1', 'chat_completions'],
  ['groq', 'Groq', 'https://api.groq.com/openai/v1', 'chat_completions'],
  ['cerebras', 'Cerebras', 'https://api.cerebras.ai/v1', 'chat_completions'],
  ['deepinfra', 'Deep Infra', 'https://api.deepinfra.com/v1/openai', 'chat_completions'],
  ['fireworks', 'Fireworks AI', 'https://api.fireworks.ai/inference/v1', 'chat_completions'],
  ['together', 'Together AI', 'https://api.together.xyz/v1', 'chat_completions'],
  ['nvidia', 'NVIDIA NIM', 'https://integrate.api.nvidia.com/v1', 'chat_completions'],
  ['huggingface', 'Hugging Face Inference Providers', 'https://router.huggingface.co/v1', 'chat_completions'],
  ['venice', 'Venice AI', 'https://api.venice.ai/api/v1', 'chat_completions'],
  ['ollama-cloud', 'Ollama Cloud', 'https://ollama.com/v1', 'ollama'],
  ['typesafe', 'TypeSafe / Jev', 'https://api.typesafe.ai/v1', 'typesafe'],
].map(([id, name, base, protocol]) => ({id, name, base, protocol, model: protocol === 'typesafe' ? 'jev-latest' : ''}));

// Compare dsh identities; opt-in native transport is provided by pi-ai. Aliases/products share
// one configuration only when their billing endpoint and auth shape are equal.
const DSH_ALIASES = {
  anthropic:['anthropic'],cerebras:['cerebras'],deepseek:['deepseek'],fireworks:['fireworks'],
  gemini:['google'],groq:['groq'],huggingface:['huggingface'],minimax:['minimax-cn'],
  mistral:['mistral'],moonshot:['moonshotai-cn'],nvidia:['nvidia'],openai:['openai'],
  'opencode-zen':['opencode'],'opencode-go':['opencode-go'],openrouter:['openrouter'],
  together:['together'],typesafe:['typesafe'],xai:['xai'],mimo:['xiaomi'],
};
const additional = [
  {id:'ant-ling',name:'Ant Ling',base:'https://api.ant-ling.com/v1'},
  {id:'baseten',name:'Baseten',base:'https://inference.baseten.co/v1'},
  {id:'azure',name:'Azure OpenAI v1',base:'https://YOUR-RESOURCE-NAME.openai.azure.com/openai/v1',protocol:'openai',template:true,hint:'填写 Azure 资源名，模型 ID 使用部署名称。此配置用于 v1 API；旧版 api-version 接口或 Entra 自动续期需使用兼容网关。'},
  {id:'cloudflare-workers-ai',name:'Cloudflare Workers AI',base:'https://api.cloudflare.com/client/v4/accounts/ACCOUNT_ID/ai/v1',template:true,hint:'填写 Cloudflare Account ID，使用具备 Workers AI 权限的 API Token；若模型列表接口不可用，可添加单个模型。'},
  {id:'kimi-coding',name:'Kimi Coding',base:'https://api.kimi.com/coding/v1',protocol:'anthropic',hint:'使用 Kimi Coding 专用 Key；模型、账号权限及 Coding Plan 限制由供应商决定。'},
  {id:'meta',name:'Meta',base:'https://api.meta.ai/v1'},
  {id:'minimax-global',name:'MiniMax (International)',base:'https://api.minimax.io/anthropic/v1',protocol:'anthropic',dshIds:['minimax']},
  {id:'moonshot-global',name:'Moonshot Kimi (International)',base:'https://api.moonshot.ai/v1',dshIds:['moonshotai']},
  {id:'qwen-token-plan',name:'Qwen Token Plan (International / Individual)',base:'https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1',dshIds:['qwen-token-plan','qwen-token-plan-individual']},
  {id:'qwen-token-plan-cn',name:'Qwen Token Plan (China)',base:'https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1'},
  {id:'vercel-ai-gateway',name:'Vercel AI Gateway',base:'https://ai-gateway.vercel.sh/v1',protocol:'openai'},
  {id:'xiaomi-token-plan-cn',name:'Xiaomi MiMo Token Plan (China)',base:'https://token-plan-cn.xiaomimimo.com/v1'},
  {id:'xiaomi-token-plan-ams',name:'Xiaomi MiMo Token Plan (Amsterdam)',base:'https://token-plan-ams.xiaomimimo.com/v1'},
  {id:'xiaomi-token-plan-sgp',name:'Xiaomi MiMo Token Plan (Singapore)',base:'https://token-plan-sgp.xiaomimimo.com/v1'},
  {id:'zai-coding',name:'Z.ai Coding Plan (International)',base:'https://api.z.ai/api/coding/paas/v4',dshIds:['zai']},
  {id:'zai-coding-cn',name:'GLM Coding Plan (China)',base:'https://open.bigmodel.cn/api/coding/paas/v4'},
  ...[
    ['amazon-bedrock','Amazon Bedrock'],['google-vertex','Google Vertex AI'],
    ['openai-codex','OpenAI Codex'],['github-copilot','GitHub Copilot'],
    ['radius','Radius'],['cloudflare-ai-gateway','Cloudflare AI Gateway'],
  ].map(([id,name])=>({id,name,base:'',nativeOnly:true,
    hint:'使用原生 Provider 运行时。填写服务密钥或展开原生凭据，按供应商要求提供区域、项目或已授权的 OAuth 凭据；OAuth 可自动刷新。'})),
];
export const PROVIDER_PRESETS = [
  ...LEGACY_PROVIDER_PRESETS.map(p=>({...p,dshIds:DSH_ALIASES[p.id]||[]})),
  ...additional.map(p=>({protocol:'chat_completions',model:'',dshIds:[p.id],...p})),
];
export function validateProviderAddress(value) {
  if(!value.trim())throw new Error('请填写模型服务或兼容网关地址');
  if(/YOUR-RESOURCE-NAME|ACCOUNT_ID|\{[^}]+\}|<[^>]+>/.test(value))throw new Error('请先替换地址中的资源名或 Account ID');
}

export function effectiveMetadata(model = {}, protocol = 'openai') {
  if (protocol === 'typesafe') return {kind: 'unknown', capabilities: ['decision'], endpoints: ['systemone']};
  const capabilities = [...new Set(model.capabilities || [])];
  const endpoints = [...new Set(model.endpoints || [])];
  let kind = model.kind || '';
  if (!endpoints.length) {
    const byCapability = {
      embedding: 'embeddings', embeddings: 'embeddings', rerank: 'rerank', reranking: 'rerank',
      image_generation: 'images/generations', image_edit: 'images/edits', image_variation: 'images/variations',
      video_generation: 'videos/generations', asr: 'audio/transcriptions', tts: 'audio/speech',
      translation: 'audio/translations', moderation: 'moderations', decision: 'systemone',
      image_embedding: 'ml/predictions', face_recognition: 'ml/predictions', person_reid: 'ml/predictions',
      ocr: 'ml/predictions', video_structure: 'ml/video/structure',
    };
    for (const cap of capabilities) if (byCapability[cap]) endpoints.push(byCapability[cap]);
    if (!endpoints.length && kind === 'embedding') endpoints.push('embeddings');
    if (!endpoints.length && (!capabilities.length || capabilities.some(c => ['chat', 'completion', 'completions', 'generate', 'generation', 'tools', 'thinking', 'vision'].includes(c))) && ['openai', 'chat_completions', 'anthropic', 'typesafe', 'ollama', 'pi'].includes(protocol)) {
      endpoints.push(protocol === 'typesafe' ? 'systemone' : protocol === 'anthropic' ? 'messages' : 'chat_completions');
    }
  }
  const capByEndpoint = {
    chat_completions: 'completion', 'chat/completions': 'completion', responses: 'completion',
    messages: 'completion', completions: 'completion', embeddings: 'embedding', rerank: 'rerank',
    'images/generations': 'image_generation', 'images/edits': 'image_edit', 'images/variations': 'image_variation',
    'videos/generations': 'video_generation', 'audio/transcriptions': 'asr', 'audio/translations': 'translation',
    'audio/speech': 'tts', moderations: 'moderation', systemone: 'decision',
  };
  for (const ep of endpoints) if (capByEndpoint[ep] && !capabilities.includes(capByEndpoint[ep])) capabilities.push(capByEndpoint[ep]);
  if (endpoints.some(ep => ['chat_completions', 'chat/completions', 'messages', 'responses', 'completions'].includes(ep))) {
    if (!capabilities.includes('chat')) capabilities.push('chat');
    if (!kind || kind === 'unknown') kind = 'chat';
  } else if ((!kind || kind === 'unknown') && endpoints.includes('embeddings')) kind = 'embedding';
  return {kind, capabilities: [...new Set(capabilities)], endpoints: [...new Set(endpoints)]};
}

export function systemOneRequest(model, text) {
  let value;
  try { value = JSON.parse(text); } catch { throw new Error('Jev 需要包含 state 和 questions 的 JSON 请求'); }
  if (!value || typeof value !== 'object' || Array.isArray(value) || !('state' in value) || !value.questions || typeof value.questions !== 'object' || Array.isArray(value.questions) || !Object.keys(value.questions).length) {
    throw new Error('Jev 需要包含 state 和 questions 的 JSON 请求');
  }
  if (value.stream) throw new Error('Jev System One 不支持流式输出');
  return {...value, model};
}
