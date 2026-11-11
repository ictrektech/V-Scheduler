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
