(() => {
  if (window.top !== window || window.soundscapeValveReady) return;
  window.soundscapeValveReady = true;
  const playing = () => [...document.querySelectorAll("audio, video")].some((media) => !media.paused && !media.ended);
  addEventListener(
    "click",
    (event) => {
      if (!window.soundscapeFreshLoad || event.defaultPrevented || event.button !== 0) return;
      if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
      const link = event.target instanceof Element ? event.target.closest("a[href]") : null;
      if (!link || (link.target && link.target !== "_self") || link.hasAttribute("download")) return;
      const url = new URL(link.href, location.href);
      if (url.origin !== location.origin) return;
      if (url.pathname === location.pathname && url.search === location.search) return;
      if (playing()) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      window.soundscapeFreshLoad = false;
      location.assign(url.href);
    },
    true,
  );
})();
