// Validate the installed-app contract: an empty configuration collects itself.
import {spawn,execFileSync} from 'node:child_process'
import {randomBytes} from 'node:crypto'
import {mkdirSync,writeFileSync,readFileSync,openSync} from 'node:fs'
import path from 'node:path'
import assert from 'node:assert/strict'
const build=process.env.SCHEDULER_BUILD||path.join(process.env.HOME,'dev/build/V-Scheduler')
const run=path.join(build,'local-install-'+Date.now());mkdirSync(run,{recursive:true,mode:0o700})
const port=Number(process.env.SCHEDULER_TEST_BASE_PORT||38520),token=randomBytes(32).toString('hex'),password=randomBytes(32).toString('hex')
const db='scheduler-local-install-'+process.pid
const config=path.join(run,'controller.json'),env=path.join(run,'postgres.env')
writeFileSync(config,JSON.stringify({principals:[{token,role:'admin',app_id:'admin'}],nodes:[],executors:[]}),{mode:0o600})
writeFileSync(env,`POSTGRES_USER=vscheduler\nPOSTGRES_DB=vscheduler\nPOSTGRES_PASSWORD=${password}\n`,{mode:0o600})
let processHandle
const delay=ms=>new Promise(r=>setTimeout(r,ms))
const binary=process.env.SCHEDULER_BINARY||path.join(build,'cargo/debug/v-scheduler')
function launch(){const fd=openSync(path.join(run,'controller.log'),'a',0o600);processHandle=spawn(binary,['controller','--config',config,'--listen',`127.0.0.1:${port}`],{env:{...process.env,SCHEDULER_EMBEDDED_AGENT:'true',SCHEDULER_DATABASE_URL:`postgres://vscheduler:${password}@127.0.0.1:${port+3}/vscheduler`},stdio:['ignore',fd,fd]})}
async function nodes(){const response=await fetch(`http://127.0.0.1:${port}/api/v1/nodes`,{headers:{Authorization:'Bearer '+token}});assert.equal(response.status,200);return (await response.json()).data}
async function ready(){for(let i=0;i<60;i++){try{const n=await nodes();if(n.length===1&&n[0].report?.cpu_cores?.length>0&&n[0].report.cpu_cores.every(c=>c.utilization_percent!=null))return n[0]}catch{}await delay(500)}throw Error('embedded collector never reported real per-core resources')}
async function stop(){if(processHandle.exitCode===null){processHandle.kill('SIGTERM');await new Promise(r=>processHandle.once('exit',r))}}
try{
 execFileSync('docker',['run','-d','--name',db,'--env-file',env,'-p',`127.0.0.1:${port+3}:5432`,'registry.ictrek.internal/library/postgres:18'],{stdio:'pipe'})
 for(let i=0;i<40;i++){try{execFileSync('docker',['exec',db,'pg_isready','-h','127.0.0.1','-U','vscheduler'],{stdio:'pipe'});break}catch{await delay(500)}}
 launch();let n=await ready();assert.equal(n.id,'local');assert.equal(n.health_state,'ready')
 assert.equal(n.report.cpu_cores.length,n.report.cpu_logical_count);assert(n.report.memory_total_mib>0)
 if(process.env.SCHEDULER_EXPECT_GPU){assert.equal(n.report.gpus[0].memory_type,process.env.SCHEDULER_EXPECT_GPU)}
 const saved=JSON.parse(readFileSync(config));assert.equal(saved.nodes.length,1);assert.equal(saved.principals.filter(p=>p.role==='worker').length,1)
 console.log('PASS: empty install automatically registers and runs its built-in collector; actual per-core CPU, RAM and GPU topology')
 const identity=JSON.parse(readFileSync(path.join(run,'local-agent.json'))).token
 await stop();launch();n=await ready();assert.equal((await nodes()).length,1);assert.equal(JSON.parse(readFileSync(path.join(run,'local-agent.json'))).token,identity)
 writeFileSync(path.join(run,'evidence.json'),JSON.stringify({cpu_used_percent:n.report.cpu_used_percent,memory_available_mib:n.report.memory_available_mib,cpu_logical_count:n.report.cpu_logical_count,cpu_cores:n.report.cpu_cores,gpus:n.report.gpus,memory_total_mib:n.report.memory_total_mib,restart_identity_preserved:true},null,2),{mode:0o600})
 console.log('PASS: application restart resumes collection with stable identity and no duplicate node or external Agent')
 console.log('Evidence: '+run)
}finally{if(processHandle)await stop();try{execFileSync('docker',['rm','-f',db],{stdio:'pipe'})}catch{}}
