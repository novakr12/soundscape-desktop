(() => {
  if (window.top !== window || window.soundscapeSmoothWheelReady) return;
  window.soundscapeSmoothWheelReady = true;
  const STIFFNESS = 0.024;
  const STEP_MS = 4;
  const LINE = 40;
  const glides = new WeakMap();
  const isWheel = (event) =>
    event.deltaMode !== 0 ||
    (event.wheelDeltaY !== 0 && event.wheelDeltaY % 120 === 0) ||
    (Number.isInteger(event.deltaY) && Math.abs(event.deltaY) >= 40);
  const canScroll = (element, down) => {
    const style = getComputedStyle(element);
    if (!/(auto|scroll)/.test(style.overflowY)) return false;
    if (element.scrollHeight <= element.clientHeight) return false;
    return down ? element.scrollTop + element.clientHeight < element.scrollHeight - 1 : element.scrollTop > 0;
  };
  const scrollerFor = (target, down) => {
    for (let element = target instanceof Element ? target : null; element && element !== document.body; element = element.parentElement) {
      if (element === document.documentElement) break;
      if (canScroll(element, down)) return element;
    }
    return document.scrollingElement;
  };
  addEventListener(
    "wheel",
    (event) => {
      if (window.soundscapeSmoothWheel === false) return;
      if (event.defaultPrevented || event.ctrlKey || event.shiftKey || event.deltaX !== 0 || event.deltaY === 0) return;
      if (!isWheel(event)) return;
      const element = scrollerFor(event.target, event.deltaY > 0);
      if (!element) return;
      const step = event.deltaMode === 1 ? event.deltaY * LINE : event.deltaMode === 2 ? event.deltaY * element.clientHeight : event.deltaY;
      const most = element.scrollHeight - element.clientHeight;
      let glide = glides.get(element);
      if (!glide) {
        glide = { at: element.scrollTop, speed: 0, target: element.scrollTop, last: 0, frame: 0 };
        glides.set(element, glide);
      }
      if (!glide.frame) {
        glide.at = element.scrollTop;
        glide.target = element.scrollTop;
        glide.speed = 0;
      }
      const target = Math.max(0, Math.min(most, glide.target + step));
      if (target === glide.target && !glide.frame) return;
      event.preventDefault();
      glide.target = target;
      if (glide.frame) return;
      glide.last = performance.now();
      const move = (now) => {
        if (Math.abs(element.scrollTop - glide.at) > 2) {
          glide.frame = 0;
          return;
        }
        let passed = Math.min(now - glide.last, 50);
        glide.last = now;
        while (passed > 0) {
          const step = Math.min(passed, STEP_MS);
          const pull = STIFFNESS * STIFFNESS * (glide.target - glide.at) - 2 * STIFFNESS * glide.speed;
          glide.speed += pull * step;
          glide.at += glide.speed * step;
          passed -= step;
        }
        glide.at = Math.max(0, Math.min(element.scrollHeight - element.clientHeight, glide.at));
        if (Math.abs(glide.target - glide.at) < 0.5 && Math.abs(glide.speed) < 0.01) {
          glide.at = glide.target;
          glide.speed = 0;
        }
        element.scrollTop = glide.at;
        glide.frame = glide.at === glide.target ? 0 : requestAnimationFrame(move);
      };
      glide.frame = requestAnimationFrame(move);
    },
    { passive: false },
  );
})();
