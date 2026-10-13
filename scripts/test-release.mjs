// Protocol simulation only: no actual Git, registry, Feishu or release calls.
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,copyFileSync,rmSync} from 'node:fs'
import {spawnSync} from 'node:child_process'
import {tmpdir} from 'node:os'
import path from 'node:path'
import assert from 'node:assert/strict'
const root=path.resolve(import.meta.dirname,'..')
const temp=mkdtempSync(path.join(process.env.SCHEDULER_BUILD||tmpdir(),'release-tests-'))
try{
 for(const scenario of ['success','dirty','local-tag','remote-public-tag','network-failure','push-failure']){
  const dir=path.join(temp,scenario);mkdirSync(path.join(dir,'ictrek.app/scripts'),{recursive:true});mkdirSync(path.join(dir,'bin'))
  copyFileSync(path.join(root,'ictrek.app/scripts/update_version.sh'),path.join(dir,'ictrek.app/scripts/update_version.sh'));writeFileSync(path.join(dir,'ictrek.app/VERSION'),'0.0.0\n')
  const log=path.join(dir,'calls.jsonl')
  writeFileSync(path.join(dir,'bin/git'),`#!/usr/bin/env python3
import os,sys,json
args=sys.argv[1:]
with open(os.environ['MOCK_LOG'],'a') as f:f.write(json.dumps(args)+'\\n')
s=os.environ['MOCK_SCENARIO']
if args[:1]==['-C']:print(os.environ['MOCK_ROOT']);sys.exit(0)
if args[:2]==['status','--porcelain']:print('?? pending.txt' if s=='dirty' else '');sys.exit(0)
if args[:2]==['branch','--show-current']:print('main');sys.exit(0)
if args[:1]==['rev-parse']:sys.exit(0 if s=='local-tag' else 1)
if args[:1]==['ls-remote']:
 if s=='network-failure':sys.exit(128)
 if s=='remote-public-tag':print('abc\\trefs/tags/v0.0.1')
 sys.exit(0)
if args[:1]==['push'] and s=='push-failure':sys.exit(1)
sys.exit(0)
`,{mode:0o755})
  const result=spawnSync('bash',[path.join(dir,'ictrek.app/scripts/update_version.sh'),'patch'],{env:{...process.env,PATH:path.join(dir,'bin')+':'+process.env.PATH,MOCK_ROOT:dir,MOCK_SCENARIO:scenario,MOCK_LOG:log},encoding:'utf8'})
  const calls=readFileSync(log,'utf8').trim().split('\n').map(JSON.parse)
  if(scenario==='success'){
   assert.equal(result.status,0,result.stderr);assert.equal(readFileSync(path.join(dir,'ictrek.app/VERSION'),'utf8'),'0.0.1\n')
   assert.deepEqual(calls.at(-1),['push','--atomic','origin','main','vos-v-scheduler-v0.0.1'])
   assert(!calls.some(c=>c[0]==='tag'&&c[1]==='v0.0.1'))
  }else if(scenario==='push-failure'){
   assert.notEqual(result.status,0);assert.equal(readFileSync(path.join(dir,'ictrek.app/VERSION'),'utf8'),'0.0.1\n');assert(result.stderr.includes('without bumping again'))
  }else{
   assert.notEqual(result.status,0);assert.equal(readFileSync(path.join(dir,'ictrek.app/VERSION'),'utf8'),'0.0.0\n');assert(!calls.some(c=>['commit','tag','push'].includes(c[0])))
  }
  console.log('PASS release '+scenario)
 }
}finally{rmSync(temp,{recursive:true,force:true})}
