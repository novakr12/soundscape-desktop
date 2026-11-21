(() => {
  const internals = navigator.servo;
  delete Navigator.prototype.servo;
  if (window.top !== window || window.soundscapeCanvasCovers) return;
  const collect =
    internals && typeof internals.garbageCollectAllContexts === "function"
      ? () => internals.garbageCollectAllContexts()
      : null;
  window.soundscapeCanvasCovers = true;
  if (!collect) return;
  let timer = 0;
  const gone = (node) =>
    node instanceof Element && (node.matches("canvas.cover-canvas") || node.querySelector("canvas.cover-canvas") !== null);
  new MutationObserver((records) => {
    if (timer || !records.some((record) => [...record.removedNodes].some(gone))) return;
    timer = setTimeout(() => {
      timer = 0;
      collect();
    }, 3000);
  }).observe(document, { childList: true, subtree: true });
})();
