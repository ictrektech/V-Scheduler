import { describe, expect, it } from 'vitest';
import { nodePowerImpact } from '../nodePower';

describe('nodePowerImpact', () => {
  it('空闲远程节点无需强制', () => {
    const impact = nodePowerImpact({ access_mode: 'ssh', busy: false });
    expect(impact).toEqual({
      local: false,
      busy: false,
      deploymentNames: [],
      needsForceConfirm: false,
    });
  });

  it('本机节点标记管理面风险', () => {
    const impact = nodePowerImpact({ access_mode: 'local', busy: false });
    expect(impact.local).toBe(true);
    expect(impact.needsForceConfirm).toBe(false);
  });

  it('多运行配置占用时列出全部名称并要求强制', () => {
    const impact = nodePowerImpact({
      access_mode: 'ssh',
      busy: true,
      busy_deployment_names: ['alpha', 'beta'],
      busy_deployment_name: 'ignored',
    });
    expect(impact.busy).toBe(true);
    expect(impact.deploymentNames).toEqual(['alpha', 'beta']);
    expect(impact.needsForceConfirm).toBe(true);
  });

  it('旧后端只有单个名称 / id 时回退', () => {
    expect(
      nodePowerImpact({ access_mode: 'ssh', busy: true, busy_deployment_name: 'legacy' }).deploymentNames,
    ).toEqual(['legacy']);
    expect(
      nodePowerImpact({ access_mode: 'ssh', busy: true, busy_deployment_id: 'dep-1' }).deploymentNames,
    ).toEqual(['dep-1']);
  });

  it('busy 为 true 但名称为空时仍要求强制', () => {
    const impact = nodePowerImpact({ access_mode: 'ssh', busy: true });
    expect(impact.busy).toBe(true);
    expect(impact.needsForceConfirm).toBe(true);
    expect(impact.deploymentNames).toEqual([]);
  });
});
