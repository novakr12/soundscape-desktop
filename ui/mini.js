const { invoke } = window.__TAURI__.core;

const message = document.getElementById("message");

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
  const share = view.length_ms && view.position_ms != null ? Math.min(1, view.position_ms / view.length_ms) : 0;
  document.getElementById("progress").style.width = `${Math.round(share * 100)}%`;
  const toggle = document.getElementById("toggle");
  toggle.textContent = view.playing ? "⏸" : "▶";
  toggle.disabled = !playing;
  document.querySelector('[data-action="previous"]').disabled = !view.can_skip;
  document.querySelector('[data-action="next"]').disabled = !view.can_next;
  const love = document.getElementById("love");
  love.textContent = view.loved ? "♥" : "♡";
  love.disabled = !view.can_love;
  love.setAttribute("aria-pressed", String(view.loved));
}

async function refresh() {
  try {
    render(await invoke("get_mini"));
  } catch (error) {
    message.textContent = String(error);
    message.hidden = false;
  }
}

for (const button of document.querySelectorAll("[data-action]")) {
  button.addEventListener("click", async () => {
    message.hidden = true;
    try {
      render(await invoke("mini_action", { action: button.dataset.action }));
    } catch (error) {
      message.textContent = String(error);
      message.hidden = false;
    }
    setTimeout(refresh, 400);
  });
}

refresh();
setInterval(refresh, 1000);
