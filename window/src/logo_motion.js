(() => {
  if (window.top !== window || window.soundscapeLogoReady) return;
  window.soundscapeLogoReady = true;
  const LENGTH = 89.65;
  const DOT_X = 51;
  const DOT_Y = 22;
  const started = new WeakSet();
  const widths = new WeakMap();
  const easeOut = (t) => 1 - (1 - t) ** 3;
  const easeInOut = (t) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);
  const progress = (elapsed, delay, length) => Math.min(Math.max((elapsed - delay) / length, 0), 1);

  const dotScale = (t) => {
    if (t <= 0) return 0;
    if (t < 0.6) return 1.5 * easeOut(t / 0.6);
    return 1.5 - 0.5 * easeOut((t - 0.6) / 0.4);
  };

  const clearLockup = (lockup) => {
    if (!lockup) return;
    lockup.style.paddingRight = "";
    const wordmark = lockup.querySelector(".wordmark");
    if (wordmark) {
      wordmark.style.maxWidth = "";
      wordmark.style.overflow = "";
      wordmark.style.whiteSpace = "";
    }
    for (const element of lockup.querySelectorAll(".tagline, .app-version")) {
      element.style.opacity = "";
    }
  };

  const draw = (svg, elapsed) => {
    const line = svg.querySelector(".logo-line");
    const area = svg.querySelector(".logo-area");
    const dot = svg.querySelector(".logo-dot");
    const drawn = easeInOut(progress(elapsed, 100, 900));
    line?.setAttribute("stroke-dasharray", `${LENGTH} ${LENGTH * 2}`);
    line?.setAttribute("stroke-dashoffset", String(LENGTH * 1.05 * (1 - drawn)));
    area?.setAttribute("opacity", String(0.25 * easeOut(progress(elapsed, 700, 500))));
    const scale = dotScale(progress(elapsed, 950, 400));
    dot?.setAttribute("transform", `translate(${DOT_X} ${DOT_Y}) scale(${scale}) translate(${-DOT_X} ${-DOT_Y})`);

    const lockup = svg.closest(".lockup.animated");
    if (!lockup) return;
    if (elapsed >= 1500) {
      clearLockup(lockup);
      return;
    }

    const wordmark = lockup.querySelector(".wordmark");
    if (wordmark) {
      const revealed = easeOut(progress(elapsed, 350, 700));
      if (revealed >= 1) {
        wordmark.style.maxWidth = "";
        wordmark.style.overflow = "";
        wordmark.style.whiteSpace = "";
      } else {
        if (!widths.has(wordmark)) {
          const measured = Math.ceil(wordmark.getBoundingClientRect().width);
          if (measured > 0) widths.set(wordmark, measured);
        }
        wordmark.style.overflow = "hidden";
        wordmark.style.whiteSpace = "nowrap";
        if (revealed <= 0) {
          wordmark.style.maxWidth = "0px";
        } else {
          const full = Math.max(widths.get(wordmark) ?? 0, wordmark.scrollWidth);
          wordmark.style.maxWidth = full > 0 ? `${(revealed * full).toFixed(2)}px` : "";
        }
      }
    }

    const shown = easeOut(progress(elapsed, 1050, 400));
    for (const element of lockup.querySelectorAll(".tagline, .app-version")) {
      element.style.opacity = shown >= 1 ? "" : String(Math.max(shown, 0.001));
    }

    lockup.style.paddingRight = `${(0.01 * (1 - progress(elapsed, 0, 1500))).toFixed(4)}px`;
  };

  const hydrated = (element) =>
    !!element && Object.keys(element).some((key) => key.startsWith("__reactFiber"));

  const animate = (svg) => {
    if (started.has(svg)) return;
    started.add(svg);
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const lockup = svg.closest(".lockup.animated");
    draw(svg, 0);
    let waited = null;
    const ready = (now) => {
      if (!svg.isConnected) {
        clearLockup(lockup);
        return;
      }
      waited ??= now;
      if (!hydrated(svg.parentElement) && now - waited < 4000) {
        requestAnimationFrame(ready);
        return;
      }
      run(svg, lockup);
    };
    requestAnimationFrame(ready);
  };

  const run = (svg, lockup) => {
    let began = null;
    const step = (now) => {
      if (!svg.isConnected) {
        clearLockup(lockup);
        return;
      }
      began ??= now;
      const elapsed = now - began;
      draw(svg, elapsed);
      if (elapsed < 1500) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  };

  const scan = (root) => {
    if (root.nodeType !== 1) return;
    const found = root.matches?.(".animated .logo svg") ? [root] : [];
    for (const svg of [...found, ...root.querySelectorAll(".animated .logo svg")]) {
      if (!svg.hasAttribute("data-soundscape-copy")) animate(svg);
    }
  };

  const start = () => {
    scan(document.documentElement);
    new MutationObserver((changes) => {
      for (const change of changes) {
        if (change.type === "attributes") scan(change.target);
        for (const added of change.addedNodes ?? []) scan(added);
      }
    }).observe(document.documentElement, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ["class"],
    });
  };

  if (document.documentElement) start();
  else document.addEventListener("DOMContentLoaded", start);
})();
