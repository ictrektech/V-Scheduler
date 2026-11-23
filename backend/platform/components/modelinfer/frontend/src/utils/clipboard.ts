/**
 * 剪贴板复制（含非安全上下文兜底）。
 *
 * `navigator.clipboard` 仅在安全上下文（https / localhost）可用；应用常以 http + 子路径托管，
 * 此时回退到临时 `textarea` + `document.execCommand('copy')`。
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  if (!text) return false;
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    const ok = document.execCommand('copy');
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}