(() => {
  if (window.top !== window || window.soundscapeSvgReady) return;
  window.soundscapeSvgReady = true;
  const SVG = "http://www.w3.org/2000/svg";
  const PAINTS = ["fill", "stroke", "stop-color", "flood-color", "lighting-color", "color", "style"];
  const NUDGE = "data-soundscape-redraw";
  const COPY = "data-soundscape-copy";
  const GAP_MS = 33;
  const KEEP_MS = 120;
  const copies = new WeakMap();
  const originals = new WeakMap();
  const families = new WeakMap();
  let stamps = 0;
  const pending = new Set();
  let resolving = false;
  let nudges = 0;

  const variable = (name, element) => {
    const own = getComputedStyle(element).getPropertyValue(name).trim();
    return own || getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  };

  const resolve = (text, element) => {
    let value = text;
    for (let round = 0; round < 8 && value.includes("var("); round += 1) {
      value = value.replace(/var\(\s*(--[\w-]+)\s*(?:,\s*([^()]*))?\)/g, (_, name, fallback) => {
        return variable(name, element) || (fallback ?? "").trim() || "currentColor";
      });
    }
    return value;
  };

  const fix = (element) => {
    if (element.namespaceURI !== SVG) return;
    if (element.localName === "text" && !families.has(element)) {
      const given = element.getAttribute("font-family") || getComputedStyle(element).fontFamily;
      families.set(element, given ? `${given}, sans-serif` : "sans-serif");
      stamp(element);
    }
    for (const name of PAINTS) {
      const value = element.getAttribute(name);
      if (!value || !value.includes("var(")) continue;
      let kept = originals.get(element);
      if (!kept) {
        kept = {};
        originals.set(element, kept);
      }
      kept[name] = value;
      resolving = true;
      element.setAttribute(name, resolve(value, element));
      resolving = false;
    }
  };

  const stamp = (text) => {
    stamps += 1;
    text.setAttribute("font-family", `soundscape-${stamps}, ${families.get(text)}`);
  };

  const fixTree = (root) => {
    if (root.nodeType !== 1) return;
    fix(root);
    for (const element of root.querySelectorAll("*")) fix(element);
  };

  const outer = (node) => {
    let found = null;
    for (let at = node.nodeType === 1 ? node : node.parentNode; at; at = at.parentNode) {
      if (at.namespaceURI === SVG && at.localName === "svg") found = at;
      if (at.namespaceURI !== SVG) break;
    }
    return found?.hasAttribute(COPY) ? null : found;
  };

  const hydrated = (element) => Object.keys(element).some((key) => key.startsWith("__reactFiber"));

  const holder = (svg) => {
    const parent = svg.parentElement;
    if (!parent || !svg.isConnected || !hydrated(parent)) return null;
    if (getComputedStyle(parent).position !== "static") return parent;
    if (parent.children.length !== 1) return null;
    parent.style.position = "relative";
    return parent;
  };

  const restamp = (root) => {
    for (const text of root.querySelectorAll("text")) {
      if (!families.has(text)) {
        const given = text.getAttribute("font-family") || "sans-serif";
        families.set(text, given.replace(/^soundscape-\d+,\s*/, ""));
      }
      stamp(text);
    }
  };

  const layer = (svg) => {
    const parent = holder(svg);
    if (!parent) return false;
    const box = svg.getBoundingClientRect();
    const frame = parent.getBoundingClientRect();
    const copy = svg.cloneNode(true);
    copy.setAttribute(COPY, "");
    copy.removeAttribute("tabindex");
    copy.setAttribute("aria-hidden", "true");
    copy.style.position = "absolute";
    copy.style.margin = "0";
    copy.style.left = `${box.left - frame.left - parent.clientLeft}px`;
    copy.style.top = `${box.top - frame.top - parent.clientTop}px`;
    copy.style.width = `${box.width}px`;
    copy.style.height = `${box.height}px`;
    copy.style.pointerEvents = "none";
    copy.style.opacity = "1";
    restamp(copy);
    const older = copies.get(svg) ?? [];
    (older.at(-1) ?? svg).after(copy);
    copies.set(svg, [...older, copy]);
    setTimeout(() => {
      if (!svg.isConnected) {
        for (const each of copies.get(svg) ?? []) each.remove();
        copies.delete(svg);
        return;
      }
      svg.style.opacity = "0";
      for (const old of older) old.remove();
      copies.set(
        svg,
        (copies.get(svg) ?? []).filter((each) => !older.includes(each)),
      );
    }, KEEP_MS);
    return true;
  };

  let last = 0;
  let waiting = false;

  const nudge = () => {
    waiting = false;
    const now = performance.now();
    if (now - last < GAP_MS) {
      waiting = true;
      setTimeout(() => requestAnimationFrame(nudge), GAP_MS - (now - last));
      return;
    }
    last = now;
    for (const svg of pending) {
      if (layer(svg)) continue;
      restamp(svg);
      nudges += 1;
      svg.setAttribute(NUDGE, String(nudges));
    }
    pending.clear();
  };

  const touched = (node) => {
    const svg = outer(node);
    if (!svg) return;
    if (pending.size === 0 && !waiting) {
      waiting = true;
      requestAnimationFrame(nudge);
    }
    pending.add(svg);
  };

  const watcher = new MutationObserver((changes) => {
    for (const change of changes) {
      if (change.type === "attributes") {
        if (change.attributeName === NUDGE) continue;
        if (change.target.closest?.(`[${COPY}]`)) continue;
        if (change.attributeName === "font-family" && change.target.localName === "text") continue;
        if (!resolving && change.target.namespaceURI === SVG) fix(change.target);
        if (change.target.namespaceURI === SVG) touched(change.target);
      } else if (change.type === "childList") {
        if (change.target.nodeType === 1 && change.target.closest?.(`[${COPY}]`)) continue;
        for (const added of change.addedNodes) {
          if (added.nodeType === 1 && added.hasAttribute(COPY)) continue;
          fixTree(added);
        }
        if (change.target.namespaceURI === SVG) touched(change.target);
      } else if (change.target.parentNode?.namespaceURI === SVG) {
        touched(change.target);
      }
    }
  });

  const refresh = () => {
    for (const element of document.querySelectorAll("svg, svg *")) {
      const kept = originals.get(element);
      if (!kept) continue;
      for (const [name, value] of Object.entries(kept)) {
        resolving = true;
        element.setAttribute(name, resolve(value, element));
        resolving = false;
      }
      touched(element);
    }
  };

  const start = () => {
    fixTree(document.documentElement);
    watcher.observe(document.documentElement, {
      subtree: true,
      childList: true,
      attributes: true,
      characterData: true,
    });
    matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => setTimeout(refresh, 0));
  };

  if (document.documentElement) start();
  else document.addEventListener("DOMContentLoaded", start);
})();
