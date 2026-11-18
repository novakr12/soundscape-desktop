(() => {
  if (window.top !== window || window.soundscapeSvgReady) return;
  window.soundscapeSvgReady = true;
  const SVG = "http://www.w3.org/2000/svg";
  const PAINTS = ["fill", "stroke", "stop-color", "flood-color", "lighting-color", "color", "style"];
  const NUDGE = "data-soundscape-redraw";
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
    return found;
  };

  const nudge = () => {
    for (const svg of pending) {
      for (const text of svg.querySelectorAll("text")) {
        if (families.has(text)) stamp(text);
      }
      nudges += 1;
      svg.setAttribute(NUDGE, String(nudges));
    }
    pending.clear();
  };

  const touched = (node) => {
    const svg = outer(node);
    if (!svg) return;
    if (pending.size === 0) requestAnimationFrame(nudge);
    pending.add(svg);
  };

  const watcher = new MutationObserver((changes) => {
    for (const change of changes) {
      if (change.type === "attributes") {
        if (change.attributeName === NUDGE) continue;
        if (change.attributeName === "font-family" && change.target.localName === "text") continue;
        if (!resolving && change.target.namespaceURI === SVG) fix(change.target);
        if (change.target.namespaceURI === SVG) touched(change.target);
      } else if (change.type === "childList") {
        for (const added of change.addedNodes) fixTree(added);
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
