import {test} from 'node:test';
import assert from 'node:assert/strict';
import {createGatewayI18n} from '../src/gateway/locale.js';
import {groupCatalog} from '../src/gateway/model-catalog.js';
test('built-in video source names translate completely and remain searchable',()=>{
 const oldDoc=globalThis.document,oldFilter=globalThis.NodeFilter;
 globalThis.document={createTreeWalker:()=>({nextNode:()=>false})};globalThis.NodeFilter={SHOW_TEXT:4};
 try {
  let locale='en';const i18n=createGatewayI18n({querySelectorAll:()=>[]},()=>locale);
  const names=['视频结构化 · 场景检测','视频结构化 · 结构化与描述'];
  const english=names.map(name=>i18n.t(name));assert.deepEqual(english,['Video structure · Scene detection','Video structure · Analysis and description']);
  assert.ok(english.every(name=>!/[\p{Script=Han}]/u.test(name)));
  const sources=names.map((name,i)=>({id:'video-'+i,name,source:'video-structure'}));
  assert.equal(groupCatalog(sources,[],'upstream','Scene detection',i18n.t).length,1);
  locale='zh-TW';assert.deepEqual(names.map(name=>i18n.t(name)),['影片結構化 · 場景偵測','影片結構化 · 結構化與描述']);
  assert.equal(groupCatalog(sources,[],'upstream','場景偵測',i18n.t).length,1);
  locale='zh-CN';assert.deepEqual(names.map(name=>i18n.t(name)),names);
  assert.equal(i18n.t("Team's private provider"),"Team's private provider");
 } finally {globalThis.document=oldDoc;globalThis.NodeFilter=oldFilter;}
});

test('static application labels have English and Traditional Chinese entries',async()=>{
 const {messages}=await import('../src/messages.ts');const fs=await import('node:fs/promises');
 const root=new URL('../src/',import.meta.url);const missing=[];
 for(const name of (await fs.readdir(root)).filter(n=>n.endsWith('.vue')&&n!=='ModelHub.vue')){
  // ModelHub owns a separate translated dictionary and fallback chain.
  const source=await fs.readFile(new URL(name,root),'utf8');
  for(const match of source.matchAll(/\bt\((["'])(.*?)\1\)/g)){
   if(/\p{Script=Han}/u.test(match[2])&&!messages[match[2]]?.every(v=>typeof v==='string'&&v.length))missing.push(name+': '+match[2]);
  }
 }
 assert.deepEqual(missing,[]);
});
