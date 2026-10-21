import {test} from 'node:test'
import assert from 'node:assert/strict'
import {matchesTask,mergeEvents,waitLabel,waitReasons,compareWaitingTasks} from '../src/task-experience.ts'
test('business IDs and executor names remain searchable with app and inferred-node filters',()=>{
 const task={task_id:'scheduler-id',app_task_id:'Image-Report-2048',app_id:'image-studio',executor_id:'image.generate',node_id:null}
 const targetNode=()=> 'node-1'
 assert(matchesTask(task,'report-2048','image-studio','node-1',targetNode))
 assert(matchesTask(task,' IMAGE.GENERATE ','','',targetNode))
 assert(!matchesTask(task,'report-2048','another-app','node-1',targetNode))
 assert(!matchesTask(task,'report-2048','','node-2',targetNode))
})
test('overlapping timeline refreshes preserve older pages and immutable event identity',()=>{
 const older=[{cursor:7,progress:{stage:'decode'}},{cursor:4,progress:{}}]
 const fresh=[{cursor:9,progress:{stage:'done'}},{cursor:7,progress:{stage:'decode'}}]
 const merged=mergeEvents(older,fresh)
 assert.deepEqual(merged.map(event=>event.cursor),[9,7,4])
 assert.deepEqual(mergeEvents(merged,fresh),merged)
 assert.equal(merged[1].progress.stage,'decode')
})
test('Chinese waiting reasons are readable and unknown future codes remain inspectable',()=>{
 assert.equal(waitLabel('WAIT_CAPACITY',text=>text),'预约容量已满')
 assert.equal(waitLabel('FUTURE_WAIT_REASON',text=>text),'FUTURE_WAIT_REASON')
 assert(waitReasons.WAIT_PRIOR_EXECUTION[1].includes('核验'))
})

test('waiting lanes follow priority, aging, arrival order and stable IDs',()=>{
 const now=Date.parse('2026-10-21T12:00:00Z')
 const task=(id,priority,wait)=>({task_id:id,created_at:new Date(now-wait).toISOString(),scheduling:{priority}})
 const high=task('high','high',0),background=task('background','background',1000)
 assert(compareWaitingTasks(high,background,now)<0)
 assert(compareWaitingTasks(task('aged','background',90000),high,now)<0)
 assert(compareWaitingTasks(task('older','background',61000),task('newer','high',1000),now)<0)
 assert(compareWaitingTasks(task('a','interactive',0),task('b','interactive',0),now)<0)
 assert(compareWaitingTasks({...background,scheduling:undefined},background,now)===0)
})
