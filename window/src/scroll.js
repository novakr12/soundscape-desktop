(() => {
  if (window.top !== window || window.soundscapeScrollReady) return;
  window.soundscapeScrollReady = true;

  const sliderPrefix = NONCE + ":slider:";
  const TRACK_PAD = 4;
  const MIN_THUMB = 36;
  const HIDE_AFTER_MS = 1200;

  let rail = null;
  let thumb = null;
  let fadeTimer = 0;
  let frame = 0;
  let dragging = null;
  let hoveringSlider = false;

  const setHoveringSlider = (next) => {
    if (hoveringSlider === next) return;
    hoveringSlider = next;
    console.debug(sliderPrefix + (next ? "1" : "0"));
  };

  const root = () => document.scrollingElement || document.documentElement;

  const metrics = () => {
    const el = root();
    if (!el) return null;
    const viewport = window.innerHeight;
    const total = el.scrollHeight;
    const maxScroll = total - viewport;
    if (maxScroll <= 2 || viewport <= TRACK_PAD * 2) return null;
    const trackHeight = viewport - TRACK_PAD * 2;
    const thumbHeight = Math.min(trackHeight, Math.max(MIN_THUMB, Math.round((viewport / total) * trackHeight)));
    const maxTop = Math.max(0, trackHeight - thumbHeight);
    const ratio = Math.min(1, Math.max(0, el.scrollTop / maxScroll));
    const thumbTop = TRACK_PAD + Math.round(ratio * maxTop);
    return { el, viewport, total, maxScroll, trackHeight, thumbHeight, maxTop, thumbTop };
  };

  const keepVisible = () => {
    if (!rail) return;
    clearTimeout(fadeTimer);
    if (dragging || rail.hasAttribute("data-hover")) return;
    fadeTimer = setTimeout(() => {
      fadeTimer = 0;
      if (rail && !dragging && !rail.hasAttribute("data-hover")) {
        rail.removeAttribute("data-active");
      }
    }, HIDE_AFTER_MS);
  };

  const ensureDom = () => {
    if (rail && rail.isConnected) return true;
    if (!document.documentElement) return false;
    rail = document.createElement("div");
    rail.id = "soundscape-scrollbar";
    rail.setAttribute("aria-hidden", "true");
    thumb = document.createElement("div");
    thumb.id = "soundscape-scrollbar-thumb";
    rail.append(thumb);

    rail.addEventListener("mouseenter", () => {
      rail.setAttribute("data-hover", "");
      if (metrics()) {
        rail.setAttribute("data-active", "");
        clearTimeout(fadeTimer);
      }
    });

    rail.addEventListener("mouseleave", () => {
      rail.removeAttribute("data-hover");
      keepVisible();
    });

    rail.addEventListener("mousedown", (event) => {
      if (event.button !== 0) return;
      const m = metrics();
      if (!m) return;
      event.preventDefault();
      event.stopPropagation();

      if (event.target === thumb) {
        dragging = { startY: event.clientY, startScroll: m.el.scrollTop };
        rail.setAttribute("data-dragging", "");
        rail.setAttribute("data-active", "");
        clearTimeout(fadeTimer);
        return;
      }

      const targetTop = Math.min(m.maxTop, Math.max(0, event.clientY - TRACK_PAD - m.thumbHeight / 2));
      const ratio = m.maxTop > 0 ? targetTop / m.maxTop : 0;
      m.el.scrollTop = ratio * m.maxScroll;
      dragging = { startY: event.clientY, startScroll: m.el.scrollTop };
      rail.setAttribute("data-dragging", "");
      rail.setAttribute("data-active", "");
      clearTimeout(fadeTimer);
      scheduleUpdate(true);
    });

    document.documentElement.append(rail);
    return true;
  };

  const update = (reveal) => {
    frame = 0;
    if (!ensureDom()) return;
    const m = metrics();
    if (!m) {
      rail.removeAttribute("data-active");
      return;
    }
    thumb.style.height = `${m.thumbHeight}px`;
    thumb.style.top = `${m.thumbTop}px`;
    if (reveal || dragging || rail.hasAttribute("data-hover")) {
      rail.setAttribute("data-active", "");
      keepVisible();
    }
  };

  const scheduleUpdate = (reveal = false) => {
    if (reveal && rail && metrics()) {
      rail.setAttribute("data-active", "");
      keepVisible();
    }
    if (!frame) frame = requestAnimationFrame(() => update(reveal));
  };

  const findSlider = (target) => {
    if (!(target instanceof Element)) return null;
    const direct = target.closest('input[type="range"]');
    if (direct instanceof HTMLInputElement) return direct;
    const group = target.closest(".volume, .player-seek, .device-seek");
    if (!group) return null;
    const inside = group.querySelector('input[type="range"]');
    return inside instanceof HTMLInputElement ? inside : null;
  };

  addEventListener(
    "mousemove",
    (event) => {
      const slider = findSlider(event.target);
      setHoveringSlider(Boolean(slider && !slider.disabled));
      if (dragging) {
        const m = metrics();
        if (!m || m.maxTop <= 0) return;
        const deltaY = event.clientY - dragging.startY;
        const scrollDelta = (deltaY / m.maxTop) * m.maxScroll;
        m.el.scrollTop = Math.min(m.maxScroll, Math.max(0, dragging.startScroll + scrollDelta));
        scheduleUpdate(true);
        return;
      }
      if (window.innerWidth - event.clientX <= 18) {
        scheduleUpdate(true);
      }
    },
    { passive: true },
  );

  const stopDragging = () => {
    if (!dragging) return;
    dragging = null;
    if (rail) {
      rail.removeAttribute("data-dragging");
      keepVisible();
    }
  };

  addEventListener("mouseup", stopDragging);
  addEventListener("blur", () => {
    stopDragging();
    setHoveringSlider(false);
  });
  addEventListener("scroll", () => scheduleUpdate(true), { passive: true });
  addEventListener("resize", () => scheduleUpdate(false));

  const setInputValue = (slider, next) => {
    const text = String(next);
    if (slider.value === text) return;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (setter) setter.call(slider, text);
    else slider.value = text;
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    slider.dispatchEvent(new Event("change", { bubbles: true }));
  };

  addEventListener(
    "wheel",
    (event) => {
      if (event.ctrlKey || event.altKey || event.metaKey) return;
      const slider = findSlider(event.target);
      if (!slider || slider.disabled) return;
      const axis = Math.abs(event.deltaY) >= Math.abs(event.deltaX) ? event.deltaY : event.deltaX;
      if (axis === 0) return;
      event.preventDefault();
      event.stopPropagation();

      const min = Number.isFinite(Number(slider.min)) && slider.min !== "" ? Number(slider.min) : 0;
      const max = Number.isFinite(Number(slider.max)) && slider.max !== "" ? Number(slider.max) : 100;
      if (max <= min) return;
      const current = Number.isFinite(Number(slider.value)) ? Number(slider.value) : min;
      const rawStep = slider.step && slider.step !== "any" ? Number(slider.step) : 0;
      const isVolume = Boolean(slider.closest(".volume") || slider.classList.contains("device-volume"));
      const step = isVolume
        ? Math.max(rawStep, (max - min) / 20)
        : rawStep > 0
          ? rawStep
          : slider.closest(".player-seek, .device-seek")
            ? Math.min(5, Math.max(1, (max - min) / 40))
            : (max - min) / 20;
      const direction = axis > 0 ? 1 : -1;
      const rawNext = Math.min(max, Math.max(min, current + direction * step));
      const next = rawStep > 0 ? Math.round((rawNext - min) / rawStep) * rawStep + min : rawNext;
      const clamped = Math.min(max, Math.max(min, Number(next.toFixed(4))));
      setInputValue(slider, clamped);
    },
    { passive: false, capture: true },
  );

  const start = () => {
    scheduleUpdate(false);
    new MutationObserver(() => scheduleUpdate(false)).observe(document.documentElement, {
      childList: true,
      subtree: true,
    });
  };

  if (document.documentElement) start();
  else document.addEventListener("DOMContentLoaded", start, { once: true });
})();
