// Normalize stream events without mistaking reasoning for the final answer.
export function playgroundStreamEvent(endpoint, payload) {
  if (payload.error) {
    throw new Error(typeof payload.error === 'string' ? payload.error : payload.error.message || JSON.stringify(payload.error));
  }
  if (endpoint === 'responses') return { text: payload.type === 'response.output_text.delta' ? payload.delta || '' : '', reasoning: '' };
  if (endpoint === 'messages') return { text: payload.delta?.text || payload.content_block?.text || '', reasoning: payload.delta?.thinking || '' };
  if (endpoint === 'completions') return { text: payload.choices?.[0]?.text || '', reasoning: '' };
  const delta = payload.choices?.[0]?.delta || {};
  return { text: delta.content || '', reasoning: delta.reasoning_content || delta.thinking || '' };
}
