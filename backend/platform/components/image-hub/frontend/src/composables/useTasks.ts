import { onUnmounted, ref } from 'vue';
import { cancelTask, deleteTask, fetchTasks, type Job } from '@/api/imageHub';

/**
 * 异步任务轮询（拉取/同步）。
 *
 * - 存在未终态任务时每 `intervalMs` 轮询一次；全部终态后停止
 * - 任务由「运行中 → 成功」时回调 `onFinished`（用于刷新镜像列表）
 */
export function useTasks(onFinished?: (job: Job) => void, intervalMs = 4000) {
  const tasks = ref<Job[]>([]);
  const loading = ref(false);
  let timer: number | undefined;
  const runningIds = new Set<string>();

  async function reload(): Promise<void> {
    loading.value = true;
    try {
      const data = await fetchTasks();
      tasks.value = data.tasks ?? [];
      syncRunningState();
    } catch {
      // 任务接口不可用时静默（不阻断主流程）
    } finally {
      loading.value = false;
    }
  }

  function syncRunningState(): void {
    const nextRunning = new Set<string>();
    for (const job of tasks.value) {
      const isRunning = job.status === 'running' || job.status === 'pending';
      if (isRunning) nextRunning.add(job.id);
      else if (runningIds.has(job.id) && job.status === 'succeeded') onFinished?.(job);
    }
    runningIds.clear();
    nextRunning.forEach((id) => runningIds.add(id));
    if (nextRunning.size > 0) startPolling();
    else stopPolling();
  }

  function startPolling(): void {
    if (timer !== undefined) return;
    timer = window.setInterval(() => {
      void reload();
    }, intervalMs);
  }

  function stopPolling(): void {
    if (timer === undefined) return;
    window.clearInterval(timer);
    timer = undefined;
  }

  /** 登记一个刚启动的任务并开始轮询 */
  function track(taskId: string): void {
    runningIds.add(taskId);
    void reload();
  }

  async function remove(id: string): Promise<void> {
    await deleteTask(id);
    await reload();
  }

  /** 请求停止运行中的任务（成功后立即刷新，状态由执行体落定） */
  async function cancel(id: string): Promise<void> {
    await cancelTask(id);
    await reload();
  }

  const runningCount = () => tasks.value.filter((t) => t.status === 'running' || t.status === 'pending').length;

  onUnmounted(stopPolling);

  return { tasks, loading, reload, track, remove, cancel, runningCount, stopPolling };
}