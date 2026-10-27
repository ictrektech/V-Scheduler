// OpenAI boundary for pi-ai. Existing transparent/native application routes do
// not use this adapter; deliberately reject fields we cannot preserve.
import {randomUUID} from 'node:crypto';
export class InputError extends Error { status = 400; }
export function contextFromChat(body, model) {
  if (!Array.isArray(body.messages) || !body.messages.length) throw new InputError('messages must be a nonempty array');
  for (const key of ['audio','prediction','logprobs','top_logprobs','logit_bias','modalities','functions','function_call']) {
    if (body[key] !== undefined) throw new InputError(`${key} is not supported by the native provider adapter; use a transparent upstream`);
  }
  if (body.n !== undefined && body.n !== 1) throw new InputError('native providers support n=1');
  const names = new Map();
  const messages = body.messages.map(m => {
    if (!m || typeof m !== 'object') throw new InputError('invalid message');
    const timestamp = Date.now();
    if (m.role === 'system' || m.role === 'developer') return {role:'system',content: textContent(m.content)};
    if (m.role === 'user') return {role:'user',content:content(m.content),timestamp};
    if (m.role === 'tool') return {role:'toolResult',toolCallId:m.tool_call_id,toolName:names.get(m.tool_call_id)||m.name||'tool',content:content(m.content),isError:false,timestamp};
    if (m.role === 'assistant') {
      const blocks = m.content == null ? [] : content(m.content);
      if(typeof m.reasoning_content==='string')blocks.unshift({type:'thinking',thinking:m.reasoning_content});
      for (const t of m.tool_calls || []) {
        if (t.type !== 'function' || !t.function?.name) throw new InputError('only function tools are supported');
        let args; try {args=JSON.parse(t.function.arguments || '{}');} catch {throw new InputError('invalid tool arguments');}
        blocks.push({type:'toolCall',id:t.id,name:t.function.name,arguments:args}); names.set(t.id,t.function.name);
      }
      return {role:'assistant',content:blocks,api:model.api,provider:model.provider,model:model.id,usage:{input:0,output:0,cacheRead:0,cacheWrite:0,totalTokens:0,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},stopReason:m.tool_calls?.length?'toolUse':'stop',timestamp};
    }
    throw new InputError('unsupported message role');
  });
  const tools=(body.tools||[]).map(t=>{
    if(t.type!=='function'||!t.function?.name)throw new InputError('only function tools are supported');
    return {name:t.function.name,description:t.function.description||'',parameters:t.function.parameters||{type:'object',properties:{}}};
  });
  return {messages,...(tools.length?{tools}:{})};
}
function textContent(value) {const c=content(value);if(c.some(b=>b.type!=='text'))throw new InputError('system messages require text');return c.map(b=>b.text).join('\n');}
function content(value) {
  if(typeof value==='string')return [{type:'text',text:value}];
  if(!Array.isArray(value))throw new InputError('message content must be text or an array');
  return value.map(b=>{
    if(b.type==='text'&&typeof b.text==='string')return {type:'text',text:b.text};
    if(b.type==='image_url') {
      const match=/^data:(image\/[a-zA-Z0-9.+-]+);base64,([A-Za-z0-9+/=\r\n]+)$/.exec(b.image_url?.url||'');
      if(!match)throw new InputError('native provider images require a base64 data URL; use a transparent upstream for remote image URLs');
      return {type:'image',mimeType:match[1],data:match[2]};
    }
    throw new InputError('unsupported content block');
  });
}
export function streamOptions(body, model) {
  const options={maxRetries:0,timeoutMs:120000,transport:'sse'};
  if(body.temperature!==undefined)options.temperature=body.temperature;
  if(body.max_completion_tokens!==undefined||body.max_tokens!==undefined)options.maxTokens=body.max_completion_tokens??body.max_tokens;
  if(body.reasoning_effort!==undefined) {
    if(!['off','minimal','low','medium','high','xhigh','max'].includes(body.reasoning_effort))throw new InputError('unsupported reasoning_effort');
    options.reasoning=body.reasoning_effort==='off'?undefined:body.reasoning_effort;
  }
  const openai=['openai-completions','openai-responses','azure-openai-responses','openai-codex-responses'].includes(model.api);
  const sampling={};
  for(const key of ['top_p','stop','frequency_penalty','presence_penalty','seed','response_format','parallel_tool_calls','tool_choice']) {
    if(body[key]!==undefined) {
      if(!openai)throw new InputError(`${key} requires an OpenAI-compatible native provider or transparent upstream`);
      if(['response_format','tool_choice'].includes(key)&&model.api!=='openai-completions')throw new InputError(`${key} requires the Chat Completions native API or a transparent upstream`);
      sampling[key]=body[key];
    }
  }
  if(Object.keys(sampling).length)options.samplingParams=sampling;
  return options;
}
export function usage(u={}) {const input=(u.input||0)+(u.cacheRead||0)+(u.cacheWrite||0);return {prompt_tokens:input,completion_tokens:u.output||0,total_tokens:input+(u.output||0),prompt_tokens_details:{cached_tokens:u.cacheRead||0}};}
export function finishReason(reason) {return ({toolUse:'tool_calls',length:'length',stop:'stop'})[reason]||'stop';}
export function completion(message, id='chatcmpl-'+randomUUID()) {
  const text=message.content.filter(b=>b.type==='text').map(b=>b.text).join('');
  const thinking=message.content.filter(b=>b.type==='thinking').map(b=>b.thinking).join('');
  const calls=message.content.filter(b=>b.type==='toolCall').map(b=>({id:b.id,type:'function',function:{name:b.name,arguments:JSON.stringify(b.arguments)}}));
  return {id,object:'chat.completion',created:Math.floor(Date.now()/1000),model:message.model,choices:[{index:0,message:{role:'assistant',content:text||null,...(thinking?{reasoning_content:thinking}:{}),...(calls.length?{tool_calls:calls}:{})},finish_reason:finishReason(message.stopReason)}],usage:usage(message.usage)};
}
export function chunk(id,model,delta,finish_reason=null,extra={}) {return {id,object:'chat.completion.chunk',created:Math.floor(Date.now()/1000),model,choices:[{index:0,delta,finish_reason}],...extra};}
