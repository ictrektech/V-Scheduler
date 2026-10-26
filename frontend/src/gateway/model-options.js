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
export const PROVIDER_PRESETS = [
  ['openrouter', 'OpenRouter', 'https://openrouter.ai/api/v1', 'openai'],
  ['deepseek', 'DeepSeek', 'https://api.deepseek.com/v1', 'chat_completions'],
  ['bailian', 'Alibaba Cloud Bailian (China)', 'https://dashscope.aliyuncs.com/compatible-mode/v1', 'chat_completions'],
  ['bailian-intl', 'Alibaba Cloud Bailian (Intl)', 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1', 'chat_completions'],
  ['zai', 'Z.ai / GLM', 'https://api.z.ai/api/paas/v4', 'chat_completions'],
  ['bigmodel', 'BigModel / GLM', 'https://open.bigmodel.cn/api/paas/v4', 'chat_completions'],
  ['moonshot', 'Moonshot Kimi', 'https://api.moonshot.cn/v1', 'chat_completions'],
  ['minimax', 'MiniMax', 'https://api.minimaxi.com/v1', 'chat_completions'],
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
    if (!endpoints.length && (!capabilities.length || capabilities.some(c => ['chat', 'completion', 'completions', 'generate', 'generation', 'tools', 'thinking', 'vision'].includes(c))) && ['openai', 'chat_completions', 'anthropic', 'typesafe', 'ollama'].includes(protocol)) {
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
