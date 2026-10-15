import {test} from 'node:test'
import assert from 'node:assert/strict'
import {occupancyMs,duration,permitBudget} from '../src/activity.ts'
const start='2026-10-14T00:00:00Z',now=Date.parse('2026-10-14T00:02:00Z')
test('old releases have unknown duration, even when the lease is in the future',()=>{
 const old={state:'released',created_at:start,lease_until:'2026-10-14T12:00:00Z'}
 assert.equal(occupancyMs(old,now),null)
 assert.equal(duration(occupancyMs(old,now)),'—')
 assert.equal(occupancyMs({...old,released_at:'2026-10-14T00:00:05Z'},now),5000)
})
test('suspect work remains occupied until confirmed stop; invalid clocks stay unknown',()=>{
 assert.equal(occupancyMs({state:'suspect',created_at:start},now),120000)
 assert.equal(occupancyMs({state:'active',created_at:'invalid'},now),null)
 assert.equal(occupancyMs({state:'released',created_at:start,released_at:'2026-10-13T00:00:00Z'},now),null)
 assert.equal(duration(Infinity),'—')
})

test('multi-node history aggregates all four budgets without changing the stored head budget',()=>{
 const resources={cpu_millicores:500,memory_mib:1024,gpu_slots:1,gpu_memory_mib:4096}
 const permit={resources,placements:[2,3,4].map(i=>({node_id:'spark-'+i,resources}))}
 assert.deepEqual(permitBudget(permit),{cpu_millicores:2000,memory_mib:4096,gpu_slots:4,gpu_memory_mib:16384})
 assert.equal(resources.cpu_millicores,500)
 assert.deepEqual(permitBudget({resources}),resources)
})
