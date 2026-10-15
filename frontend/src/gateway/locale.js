import { VRouterMessages } from './messages.js';

// The combined application owns one locale controller. Scope legacy gateway
// bindings to its workspace so language changes preserve forms and chat state.
export function createGatewayI18n(root, getLocale) {
  const labels = [], bindings = new Map();
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  while (walker.nextNode()) {
    const node = walker.currentNode;
    if (/\p{Script=Han}/u.test(node.nodeValue) && !node.parentElement?.closest('style,script,#localeSelect')) labels.push([node, null, node.nodeValue]);
  }
  for (const node of root.querySelectorAll('[title],[placeholder],[aria-label]')) {
    for (const attr of ['title', 'placeholder', 'aria-label']) {
      const source = node.getAttribute(attr);
      if (source && /\p{Script=Han}/u.test(source)) labels.push([node, attr, source]);
    }
  }
  function t(source) {
    const locale = getLocale();
    if (locale === 'zh-CN') return source;
    const value = String(source), trimmed = value.trim();
    const exact = VRouterMessages[trimmed]?.[locale];
    return exact ? value.replace(trimmed, exact) : value.replace(/[\p{Script=Han}]+/gu, part => VRouterMessages[part]?.[locale] || part);
  }
  function bind(element, property, render) {
    const properties = bindings.get(element) || new Map();
    properties.set(property, render); bindings.set(element, properties);
    return element[property] = render();
  }
  function apply() {
    for (const [node, attr, source] of labels) {
      if (attr) node.setAttribute(attr, t(source)); else node.nodeValue = t(source);
    }
    for (const [node, properties] of bindings) {
      if (!node.isConnected) { bindings.delete(node); continue; }
      for (const [property, render] of properties) node[property] = render();
    }
  }
  return { t, bind, apply, get locale() { return getLocale(); } };
}
