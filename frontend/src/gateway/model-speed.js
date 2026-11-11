// Use actual time spacing and break the line where that metric is unavailable.
export function speedSeries(points, metric, width = 700, height = 130) {
  const times = points.map(point => Date.parse(point.time));
  const start = Math.min(...times.filter(Number.isFinite));
  const end = Math.max(...times.filter(Number.isFinite));
  const max = Math.max(0, ...points.map(point => point[metric]).filter(Number.isFinite));
  const segments = [];
  let segment = [];
  const dots = [];
  points.forEach((point, index) => {
    const value = point[metric], time = times[index];
    if (!Number.isFinite(value) || !Number.isFinite(time)) {
      if (segment.length) segments.push(segment);
      segment = [];
      return;
    }
    const x = end === start ? width / 2 : 8 + (time - start) / (end - start) * (width - 16);
    const y = height - 8 - (max > 0 ? value / max : 0) * (height - 16);
    const dot = {x: x.toFixed(2), y: y.toFixed(2), index, value};
    segment.push(dot);
    dots.push(dot);
  });
  if (segment.length) segments.push(segment);
  return {dots, segments, max};
}

// One readout per chart, shared by pointer, keyboard focus and touch.
export function bindSpeedTooltips(root) {
  function marker(event) {
    return event.target.closest?.('[data-speed-point]');
  }
  function show(event) {
    const point = marker(event);
    if (!point) return;
    const chart = point.closest('.speed-chart-area');
    const tip = chart.querySelector('.speed-tooltip');
    tip.querySelector('strong').textContent = point.dataset.speedPoint;
    tip.querySelector('span').textContent = point.dataset.speedContext;
    tip.hidden = false;
    const bounds = chart.getBoundingClientRect();
    const dot = point.getBoundingClientRect();
    const x = dot.left + dot.width / 2 - bounds.left;
    const above = dot.top - bounds.top - tip.offsetHeight - 8;
    tip.style.left = Math.max(0, Math.min(x - tip.offsetWidth / 2, bounds.width - tip.offsetWidth)) + 'px';
    tip.style.top = (above >= 0 ? above : dot.bottom - bounds.top + 8) + 'px';
    chart.querySelector('[data-active-point]')?.removeAttribute('data-active-point');
    point.setAttribute('data-active-point', '');
  }
  function hide(event) {
    const point = marker(event);
    if (!point?.hasAttribute('data-active-point')) return;
    point.removeAttribute('data-active-point');
    point.closest('.speed-chart-area').querySelector('.speed-tooltip').hidden = true;
  }
  for (const name of ['pointerover', 'focusin', 'click']) root.addEventListener(name, show);
  for (const name of ['pointerout', 'focusout']) root.addEventListener(name, hide);
}
