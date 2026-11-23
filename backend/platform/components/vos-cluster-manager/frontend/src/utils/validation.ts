/**
 * 节点录入/创建向导校验纯函数（可单测，不依赖 DOM）。
 * 后端契约：node_ip 合法 IPv4、admin_password 长度 ≥6（对齐 Phase 4 错误码 20501）。
 */

/** 合法 IPv4（4 段 0-255，无前导零语义限制——同后端解析允许的形式） */
export function isValidIpv4(ip: string): boolean {
  const parts = ip.split('.');
  if (parts.length !== 4) return false;
  return parts.every((p) => {
    if (!/^\d{1,3}$/.test(p)) return false;
    const n = Number(p);
    return n >= 0 && n <= 255;
  });
}

/** 管理员密码最小长度（对齐后端 20501 校验：长度 ≥6） */
export const MIN_PASSWORD_LEN = 6;

/** 密码是否合法（非空且长度 ≥6） */
export function validatePassword(pwd: string): boolean {
  return pwd.length >= MIN_PASSWORD_LEN;
}

/** 集群名称是否合法（非空；后端 create 校验 trim 非空） */
export function isValidClusterName(name: string): boolean {
  return name.trim().length > 0;
}

/** 节点录入行校验结果 */
export interface NodeRowValidation {
  ipOk: boolean;
  pwdOk: boolean;
  ok: boolean;
}

/** 单节点录入行整体校验 */
export function validateNodeRow(nodeIp: string, password: string): NodeRowValidation {
  const ipOk = isValidIpv4(nodeIp);
  const pwdOk = validatePassword(password);
  return { ipOk, pwdOk, ok: ipOk && pwdOk };
}