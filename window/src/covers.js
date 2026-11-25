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
  let again = false;
  const schedule = () => {
    if (timer) {
      again = true;
      return;
    }
    timer = setTimeout(() => {
      timer = 0;
      collect();
      if (again) {
        again = false;
        schedule();
      }
    }, 1000);
  };
  const gone = (node) => {
    if (!(node instanceof Element)) return false;
    if (node.localName === "canvas") return node.classList.contains("cover-canvas");
    return node.firstElementChild !== null && node.querySelector("canvas.cover-canvas") !== null;
  };
  new MutationObserver((records) => {
    if (timer && again) return;
    let found = false;
    for (const record of records) {
      for (const node of record.removedNodes) {
        if (gone(node)) {
          found = true;
          break;
        }
      }
      if (found) break;
    }
    if (found) schedule();
  }).observe(document, { childList: true, subtree: true });
})();
