(() => {
  if (window.top !== window) return;
  const show = () => {
    const box = document.createElement("div");
    box.setAttribute("aria-hidden", "true");
    box.style.cssText = "position:fixed;top:6px;right:6px;z-index:2147483647;padding:2px 6px;border-radius:4px;background:rgba(0,0,0,.75);color:#00ff66;font:12px monospace;pointer-events:none";
    box.textContent = "… fps";
    document.documentElement.append(box);
    let frames = 0;
    let since = performance.now();
    const tick = (now) => {
      frames += 1;
      if (now - since >= 500) {
        box.textContent = `${Math.round((frames * 1000) / (now - since))} fps`;
        frames = 0;
        since = now;
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  };
  if (document.documentElement) show();
  else document.addEventListener("DOMContentLoaded", show);
})();
