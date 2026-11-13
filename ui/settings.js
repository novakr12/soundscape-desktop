const { invoke } = window.__TAURI__.core;

const message = document.getElementById("message");
let busy = false;

function plural(count, word) {
  return `${count.toLocaleString("en")} ${word}${count === 1 ? "" : "s"}`;
}

function showError(error) {
  message.textContent = String(error);
  message.hidden = false;
}

function render(settings) {
  const account = document.getElementById("account");
  account.textContent = settings.connected
    ? `Connected to ${settings.server} as ${settings.user}.`
    : settings.server
      ? `This app isn't connected to ${settings.server} yet, so it can't send what you play.`
      : "Not connected to a server.";
  document.getElementById("signed-out").hidden = !settings.signed_out;
  document.querySelector('[data-action="sign-out"]').disabled = !settings.connected;
  document.querySelector('[data-action="connect"]').hidden = !settings.server || (settings.connected && !settings.signed_out);

  for (const input of document.querySelectorAll("[data-option]")) {
    input.checked = settings[input.dataset.option];
  }

  document.getElementById("listening").textContent = settings.listening
    ? `Listening now: ${settings.listening}`
    : settings.scrobbling
      ? "Nothing playing right now."
      : "Scrobbling is paused.";

  const list = document.getElementById("players");
  list.replaceChildren(
    ...(settings.players.length === 0
      ? [Object.assign(document.createElement("li"), { className: "muted", textContent: "Start a music player and it shows up here." })]
      : settings.players.map((player) => {
          const item = document.createElement("li");
          const label = document.createElement("label");
          label.className = "switch";
          const input = document.createElement("input");
          input.type = "checkbox";
          input.checked = player.enabled;
          input.addEventListener("change", () => act(() => invoke("set_player", { id: player.id, value: input.checked })));
          const name = document.createElement("span");
          name.textContent = player.name;
          label.append(input, name);
          if (!player.open) {
            const closed = document.createElement("small");
            closed.className = "muted";
            closed.textContent = "not open";
            label.append(closed);
          }
          item.append(label);
          return item;
        })),
  );

  document.getElementById("waiting").textContent = settings.waiting
    ? `${plural(settings.waiting, "play")} waiting to be sent.`
    : "No plays waiting.";
  document.getElementById("send-now").hidden = settings.waiting === 0;
}

async function act(call) {
  busy = true;
  message.hidden = true;
  try {
    render(await call());
  } catch (error) {
    showError(error);
    render(await invoke("get_settings"));
  } finally {
    busy = false;
  }
}

for (const input of document.querySelectorAll("[data-option]")) {
  input.addEventListener("change", () => act(() => invoke("set_option", { name: input.dataset.option, value: input.checked })));
}

for (const button of document.querySelectorAll("[data-action]")) {
  button.addEventListener("click", async () => {
    const action = button.dataset.action;
    if (action === "sign-out" && !window.confirm("Sign out? The app's token is revoked and it stops sending plays.")) return;
    await invoke("settings_action", { action });
  });
}

document.getElementById("send-now").addEventListener("click", () => act(() => invoke("send_now")));

document.getElementById("test-notification").addEventListener("click", async () => {
  message.hidden = true;
  try {
    await invoke("test_notification");
  } catch (error) {
    showError(error);
  }
});

async function refresh() {
  if (busy || document.hidden) return;
  try {
    render(await invoke("get_settings"));
  } catch (error) {
    showError(error);
  }
}

refresh();
setInterval(refresh, 3000);
