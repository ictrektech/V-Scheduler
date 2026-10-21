export type Task={scheduling?:{priority?:string},app_task_id?:string,executor_id?:string,task_id:string,app_id:string,status:string,node_id:string|null,version:number,attempt_count:number,can_retry:boolean,progress:{percent?:number,stage?:string,message?:string},wait_reasons:string[],created_at:string,result:unknown}
export type Attempt={attempt_id:string,fence:number,node_id:string,status:string,result:unknown,created_at:string,stopped_at:string|null}
export type TaskEvent={cursor:number,task_id:string,status:string,version:number,progress:Task['progress'],created_at:string}
export const waitReasons:Record<string,[string,string]>={
 WAIT_NODE_OFFLINE:['节点离线','等待节点恢复连接；请检查目标设备和采集进程。'],
 WAIT_NODE_DRAINING:['节点正在排空','节点暂停接收新任务；需要时可在资源卡恢复调度。'],
 WAIT_TELEMETRY_STALE:['资源采样已过期','等待新鲜的资源采样，避免按过期数据分配任务。'],
 WAIT_EXECUTOR_UNAVAILABLE:['执行器不可用','请检查目标节点是否已安装并登记对应执行器。'],
 WAIT_EXECUTOR_UNHEALTHY:['执行器健康检查失败','请检查业务服务是否启动，以及健康检查地址是否可达。'],
 WAIT_CAPACITY:['预约容量已满','等待已预约任务确认停止并释放资源。'],
 WAIT_OBSERVED_PRESSURE:['设备资源压力较高','实际资源余量不足；请等待外部负载降低或释放已有预约。'],
 WAIT_PRIOR_EXECUTION:['等待前一次执行停止','旧执行未确认停止，资源仍被保留。请先核验执行端。'],
 WAIT_RETRY_BACKOFF:['等待自动重试','任务正在退避等待；到期后会重新排队。'],
 WAIT_RESOURCE_OR_NODE:['等待资源或节点','等待可用节点与资源预算，不代表任务失败。'],
}
export function waitLabel(code:string,t:(value:string)=>string){return t(waitReasons[code]?.[0]||code)}
export function matchesTask(task:Task,query:string,app:string,node:string,targetNode:(task:Task)=>string){
 const needle=query.trim().toLowerCase()
 return (!app||task.app_id===app)&&(!node||targetNode(task)===node)&&(!needle||[task.task_id,task.app_task_id,task.app_id,task.executor_id].filter(Boolean).join(' ').toLowerCase().includes(needle))
}
export function mergeEvents(previous:TaskEvent[],incoming:TaskEvent[]){
 return [...new Map([...previous,...incoming].map(event=>[event.cursor,event])).values()].sort((a,b)=>b.cursor-a.cursor)
}

export function compareWaitingTasks(a:Task,b:Task,now:number){
 const score=(task:Task)=>({background:0,interactive:1,high:2}[task.scheduling?.priority||'background']||0)+Math.max(0,Math.floor((now-Date.parse(task.created_at))/30000)||0)
 return score(b)-score(a)||a.created_at.localeCompare(b.created_at)||a.task_id.localeCompare(b.task_id)
}
