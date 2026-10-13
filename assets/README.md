# V-Scheduler identity

The canonical source is `icon.svg`, authored on a 100-unit grid. Three equal 26 × 12 task bars share x=18 and centers y=30/50/70. The selected task and admission arrow use jade; the arrow and execution frame share y=50. A play symbol identifies execution. The warm ivory tile, ink-blue queue and jade active path use flat fills, with identical 3-unit strokes and deliberate negative space.

The frontend imports this SVG directly, so the production bundle tracks graphic changes without stale image caching.

Export a 1024px PNG from this SVG for VOS `ictrek.app/src/icon.png` and `frontend/public/icon.png`. Preserve the SVG for future size/color exports. No generated bitmap is the geometry source.
