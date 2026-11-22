(() => {
  const nonce = NONCE;
  const items = ITEMS;
  const rect = RECT;
  document.getElementById("soundscape-picker")?.remove();
  const ratio = window.devicePixelRatio || 1;
  const dark = matchMedia("(prefers-color-scheme: dark)").matches;
  const shade = document.createElement("div");
  shade.id = "soundscape-picker";
  shade.style.cssText = "position:fixed;inset:0;z-index:2147483647";
  const list = document.createElement("div");
  const left = rect.x / ratio;
  const below = (rect.y + rect.height) / ratio;
  const above = rect.y / ratio;
  const room = innerHeight - below - 8;
  const upward = room < 160 && above > room;
  const tallest = Math.max(120, upward ? above - 8 : room);
  list.style.cssText = `position:absolute;left:${left}px;${upward ? `bottom:${innerHeight - above}px` : `top:${below}px`};min-width:${rect.width / ratio}px;max-height:${tallest}px;overflow:auto;padding:4px;border-radius:8px;box-shadow:0 8px 24px rgba(0,0,0,.35);font:14px system-ui,sans-serif;background:${dark ? "#1e1e1e" : "#ffffff"};color:${dark ? "#f2f2f2" : "#111111"};border:1px solid ${dark ? "#333333" : "#dddddd"}`;
  const close = (event) => {
    if (event.key === "Escape") finish("none");
  };
  const finish = (value) => {
    removeEventListener("keydown", close, true);
    shade.remove();
    console.log(`${nonce}:select:${value}`);
  };
  for (const item of items) {
    const row = document.createElement("div");
    row.textContent = item.label;
    if (item.group) {
      row.style.cssText = "padding:6px 10px;font-weight:600;opacity:.6";
    } else {
      row.style.cssText = `padding:6px 10px;border-radius:6px;white-space:nowrap;cursor:${item.disabled ? "default" : "pointer"};opacity:${item.disabled ? 0.45 : 1};${item.chosen ? "font-weight:600" : ""}`;
      if (!item.disabled) {
        row.addEventListener("mouseenter", () => {
          row.style.background = dark ? "#2e2e2e" : "#eeeeee";
        });
        row.addEventListener("mouseleave", () => {
          row.style.background = "";
        });
        row.addEventListener("click", (event) => {
          event.stopPropagation();
          finish(item.id);
        });
      }
    }
    list.append(row);
  }
  shade.addEventListener("mousedown", (event) => event.preventDefault());
  shade.addEventListener("pointerdown", (event) => event.preventDefault());
  shade.addEventListener("click", () => finish("none"));
  addEventListener("keydown", close, true);
  shade.append(list);
  document.documentElement.append(shade);
})()
