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

export {PROVIDER_PRESETS,replaceProviderCatalog} from './model-provider-presets.js';

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
