export type Timing={samples:number,p50_ms:number|null,p95_ms:number|null}
export type Statistics={
 window:{hours:number,from:string,until:string},min_p95_samples:number,
 first_queue_wait:Timing,
 attempts:{succeeded:number,failed:number,cancelled:number,failure_rate:number|null,occupancy:Timing},
 permits:{occupancy:Timing}
}
export type Resources={cpu_millicores:number,memory_mib:number,gpu_slots:number,gpu_memory_mib:number}
export type Permit={permit_id:string,app_id:string,executor_id:string,node_id:string,placements?:{node_id:string,resources:Resources}[]|null,state:string,model_id?:string|null,parent_task_id?:string|null,resources:Resources,delegated_resources?:Resources,evidence?:string|null,created_at:string,released_at?:string|null}

// Display an aggregate only; admission still checks each physical node separately.
export function permitBudget(permit:Permit):Resources {
 const base=permit.parent_task_id&&permit.delegated_resources?permit.delegated_resources:permit.resources
 return (permit.placements||[]).reduce((sum,part)=>({cpu_millicores:(sum.cpu_millicores||0)+(part.resources.cpu_millicores||0),memory_mib:(sum.memory_mib||0)+(part.resources.memory_mib||0),gpu_slots:(sum.gpu_slots||0)+(part.resources.gpu_slots||0),gpu_memory_mib:(sum.gpu_memory_mib||0)+(part.resources.gpu_memory_mib||0)}),{...base})
}

// A lease deadline may still be in the future after the work stops. It never
// supplies an end time for historical records or for percentile samples.
export function occupancyMs(permit:Permit,now:number):number|null {
 const start=Date.parse(permit.created_at)
 const end=permit.state==='released'?permit.released_at?Date.parse(permit.released_at):NaN:['active','suspect'].includes(permit.state)?now:NaN
 return Number.isFinite(start)&&Number.isFinite(end)&&end>=start?end-start:null
}
export function duration(ms:number|null|undefined):string {
 if(ms==null||!Number.isFinite(ms)||ms<0)return '—'
 if(ms<1000)return `${Math.round(ms)} ms`
 if(ms<60000)return `${(ms/1000).toFixed(ms<10000?1:0)} s`
 if(ms<3600000)return `${Math.floor(ms/60000)} m ${Math.floor(ms%60000/1000)} s`
 return `${Math.floor(ms/3600000)} h ${Math.floor(ms%3600000/60000)} m`
}
