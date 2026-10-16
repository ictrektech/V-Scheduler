import {test} from 'node:test';
import assert from 'node:assert/strict';
import {playgroundStreamEvent} from '../src/gateway/playground-stream.js';

test('Qwen reasoning streams remain distinct from answer content', () => {
  assert.deepEqual(playgroundStreamEvent('chat', {choices:[{delta:{reasoning_content:'thinking'}}]}), {text:'',reasoning:'thinking'});
  assert.deepEqual(playgroundStreamEvent('chat', {choices:[{delta:{content:'你好'}}]}), {text:'你好',reasoning:''});
});
test('errors after HTTP 200 are reported instead of swallowed', () => {
  assert.throws(() => playgroundStreamEvent('chat', {error:{message:'backend stopped'}}), /backend stopped/);
});
test('Responses metadata is not rendered as answer text', () => {
  assert.deepEqual(playgroundStreamEvent('responses', {type:'response.output_text.delta',delta:'hello'}), {text:'hello',reasoning:''});
  assert.equal(playgroundStreamEvent('responses', {type:'response.reasoning_summary_text.delta',delta:'reasoning'}).text, '');
});
