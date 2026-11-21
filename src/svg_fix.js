// Servo SVG rendering workarounds for charts (Recharts) and dynamic themed icons:
// 1. Hide transient Recharts hover overlays that trigger expensive full-SVG repaints.
// 2. Inline CSS custom properties (`var(--...)`) on SVG paint attributes, and re-resolve them
//    on color-scheme changes, because Servo does not always resolve/update CSS variables in SVG.
// 3. Bust Servo's SVG `<text>` glyph cache by prefixing a unique dummy font family per mutation.
// 4. Double-buffer mutated `<svg>` elements using a cloned static overlay once React has hydrated
//    the parent container so chart updates do not flash blank frames.
(() => {
  if (window.top !== window || window.soundscapeSvgReady) return;
  window.soundscapeSvgReady = true;
  const SVG = "http://www.w3.org/2000/svg";
  const PAINTS = ["fill", "stroke", "stop-color", "flood-color", "lighting-color", "color", "style"];
  const NUDGE = "data-soundscape-redraw";
  const COPY = "data-soundscape-copy";
  const GAP_MS = 16;
  const KEEP_MS = 80;
  const copies = new WeakMap();
  // Recharts hover overlays cause constant SVG re-rasterization on pointer moves in Servo.
  const HOVER = ".recharts-tooltip-cursor, .recharts-active-dot, .recharts-active-bar";
  const drawn = new WeakMap();

  const SKIPPED = new Set([NUDGE, "style", "font-family"]);

  // Fingerprint meaningful SVG structure so unrelated/internal attribute tweaks do not re-clone.
  const content = (svg) => {
    const parts = [];
    const walk = (node) => {
      if (node.nodeType === 3) {
        parts.push(node.data);
        return;
      }
      if (node.nodeType !== 1 || (node !== svg && node.matches(HOVER))) return;
      parts.push("<", node.localName);
      for (const attribute of node.attributes) {
        if (SKIPPED.has(attribute.name)) continue;
        parts.push(" ", attribute.name, "=", attribute.value);
      }
      parts.push(">");
      for (let child = node.firstChild; child; child = child.nextSibling) walk(child);
      parts.push("/");
    };
    walk(svg);
    return parts.join("");
  };

  const changed = (svg) => {
    const now = content(svg);
    if (drawn.get(svg) === now) return false;
    drawn.set(svg, now);
    return true;
  };
  const originals = new WeakMap();
  const families = new WeakMap();
  let stamps = 0;
  const tokens = new WeakMap();
  const pending = new Set();
  let resolving = false;
  let nudges = 0;

  const variable = (name, element) => {
    const own = getComputedStyle(element).getPropertyValue(name).trim();
    return own || getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  };

  // Iteratively expand nested CSS custom properties (`var(--x, fallback)`) into concrete values.
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
    if (element.matches?.(HOVER) && element.getAttribute("display") !== "none") {
      element.setAttribute("display", "none");
    }
    // Tag each SVG <text> node with a per-SVG dummy font prefix to defeat Servo's glyph cache.
    if (element.localName === "text" && !families.has(element)) {
      const given = element.getAttribute("font-family") || getComputedStyle(element).fontFamily;
      families.set(element, given ? `${given}, sans-serif` : "sans-serif");
      const svg = element.ownerSVGElement ?? element.closest("svg");
      let token = svg ? tokens.get(svg) : undefined;
      if (token === undefined) {
        stamps += 1;
        token = stamps;
        if (svg) tokens.set(svg, token);
      }
      stamp(element, token);
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

  const stamp = (text, token) => {
    text.setAttribute("font-family", `soundscape-${token}, ${families.get(text)}`);
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

  // Inserting sibling clone overlays before React finishes SSR hydration would cause hydration
  // mismatches, so wait until React attaches its internal `__reactFiber` property to the parent.
  const hydrated = (element) => Object.keys(element).some((key) => key.startsWith("__reactFiber"));

  const holder = (svg) => {
    const parent = svg.parentElement;
    if (!parent || !svg.isConnected || !hydrated(parent)) return null;
    if (getComputedStyle(parent).position !== "static") return parent;
    if (parent.children.length !== 1) return null;
    parent.style.position = "relative";
    return parent;
  };

  // Bump the dummy font-family prefix so Servo discards cached text bitmaps on redraw.
  const restamp = (root) => {
    stamps += 1;
    tokens.set(root, stamps);
    for (const text of root.querySelectorAll("text")) {
      if (!families.has(text)) {
        const given = text.getAttribute("font-family") || "sans-serif";
        families.set(text, given.replace(/^soundscape-\d+,\s*/, ""));
      }
      stamp(text, stamps);
    }
  };

  // Paint an updated static clone over the live SVG, then hide the live SVG and drop older
  // clones after `KEEP_MS` so Servo swaps frames cleanly without flashing blank space.
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
    if (older.length > 0) older.at(-1).after(copy);
    else parent.prepend(copy);
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

  // Throttle repaints to ~60 fps; fall back to bumping `NUDGE` when cloning is not possible.
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
      if (!changed(svg)) continue;
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

  // Re-evaluate saved `var(--...)` paint attributes when the OS dark/light theme changes.
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
