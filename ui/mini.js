const { invoke } = window.__TAURI__.core;

const message = document.getElementById("message");
const seek = document.getElementById("seek");
const REPEAT_LABELS = { off: "Repeat is off", all: "Repeating the queue", one: "Repeating this song" };
let dragging = false;

function clock(ms) {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return "0:00";
  const seconds = Math.floor(ms / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

function showMode(button, on, label, shown) {
  button.hidden = !shown;
  button.setAttribute("aria-pressed", String(on));
  button.title = label;
}

function render(view) {
  const playing = view.kind !== "none";
  document.getElementById("title").textContent = playing ? view.title : "Nothing playing";
  document.getElementById("artist").textContent = playing ? `${view.artist} · ${view.source}` : "Start a song in any player.";
  const art = document.getElementById("art");
  if (view.art) {
    if (art.getAttribute("src") !== view.art) art.src = view.art;
    art.hidden = false;
  } else {
    art.hidden = true;
    art.removeAttribute("src");
  }
  const length = view.length_ms ?? 0;
  const position = Math.min(view.position_ms ?? 0, length || Number.MAX_SAFE_INTEGER);
  document.getElementById("length").textContent = clock(length);
  seek.disabled = !view.can_seek || length === 0;
  if (!dragging) {
    seek.max = String(Math.max(length / 1000, 1));
    seek.value = String(position / 1000);
    document.getElementById("time").textContent = clock(position);
  }
  seek.style.setProperty("--fill", `${length ? Math.round((Number(seek.value) / Number(seek.max)) * 100) : 0}%`);
  const toggle = document.getElementById("toggle");
  toggle.textContent = view.playing ? "⏸" : "▶";
  toggle.disabled = !playing;
  document.querySelector('[data-action="previous"]').disabled = !view.can_skip;
  document.querySelector('[data-action="next"]').disabled = !view.can_next;
  showMode(document.getElementById("shuffle"), view.shuffle === true, view.shuffle ? "Shuffle is on" : "Shuffle is off", view.shuffle != null);
  const repeat = view.repeat ?? "off";
  showMode(document.getElementById("repeat"), repeat !== "off", REPEAT_LABELS[repeat] ?? REPEAT_LABELS.off, view.repeat != null);
  document.getElementById("repeat-one").setAttribute("visibility", repeat === "one" ? "visible" : "hidden");
  const next = document.getElementById("next");
  next.textContent = view.up_next ? `Up next: ${view.up_next}` : "";
  next.hidden = !view.up_next;
  const love = document.getElementById("love");
  love.textContent = view.loved ? "♥" : "♡";
  love.disabled = !view.can_love;
  love.setAttribute("aria-pressed", String(view.loved));
}

function failed(error) {
  message.textContent = String(error);
  message.hidden = false;
}

async function refresh() {
  try {
    render(await invoke("get_mini"));
  } catch (error) {
    failed(error);
  }
}

async function act(action, seconds) {
  message.hidden = true;
  try {
    render(await invoke("mini_action", { action, seconds }));
  } catch (error) {
    failed(error);
  }
  setTimeout(refresh, 400);
}

for (const button of document.querySelectorAll("[data-action]")) {
  button.addEventListener("click", () => act(button.dataset.action));
}

seek.addEventListener("input", () => {
  dragging = true;
  document.getElementById("time").textContent = clock(Number(seek.value) * 1000);
  seek.style.setProperty("--fill", `${Math.round((Number(seek.value) / Number(seek.max)) * 100)}%`);
});

seek.addEventListener("change", async () => {
  await act("seek", Number(seek.value));
  dragging = false;
});

refresh();
setInterval(refresh, 1000);
