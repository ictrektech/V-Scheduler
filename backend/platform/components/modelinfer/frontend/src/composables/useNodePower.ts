/** 节点关机 / 重启的共享确认交互（列表与详情页复用）。 */
import { h } from 'vue';
import { message, Modal } from 'ant-design-vue';
import { useI18n } from 'vue-i18n';
import { powerNode, type NodeItem } from '@/api/nodes';
import { resolveApiErrorMessage } from '@/i18n/api-error';
import { nodePowerImpact, type NodePowerAction } from '@/utils/nodePower';

export function useNodePower() {
  const { t } = useI18n();

  function confirmPower(node: NodeItem, action: NodePowerAction) {
    const impact = nodePowerImpact(node);
    const force = impact.needsForceConfirm;
    const lines = [t(`nodes.power.confirm.${action}`, { name: node.name, host: node.host })];
    if (impact.local) lines.push(t('nodes.power.localWarning'));
    if (action === 'shutdown') lines.push(t('nodes.power.shutdownWarning'));
    if (impact.busy) {
      const names = impact.deploymentNames.join(t('nodes.nodeJoin')) || t('common.none');
      lines.push(t('nodes.power.busyWarning', { names }));
    }

    Modal.confirm({
      title: t(`nodes.power.title.${action}`),
      content: h('div', { style: 'white-space: pre-line' }, lines.join('\n')),
      okText: force ? t(`nodes.power.force.${action}`) : t(`nodes.power.ok.${action}`),
      okType: 'danger',
      cancelText: t('common.cancel'),
      async onOk() {
        try {
          await powerNode(node.id, { action, force });
          message.success(t(`nodes.power.sent.${action}`));
        } catch (e) {
          message.error(resolveApiErrorMessage(e));
          // 失败时保留弹窗，便于用户修正后重试。
          throw e;
        }
      },
    });
  }

  return { confirmPower };
}
