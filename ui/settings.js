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

  renderFixes(settings);

  document.getElementById("waiting").textContent = settings.waiting
    ? `${plural(settings.waiting, "play")} waiting to be sent.`
    : "No plays waiting.";
  document.getElementById("send-now").hidden = settings.waiting === 0;
}

const fixForm = document.getElementById("fix-form");
const fixArtist = document.getElementById("fix-artist");
const fixTitle = document.getElementById("fix-title");
const fixEvery = document.getElementById("fix-every");
let fixSong = null;
let fixShown = null;
let fixEdited = false;

function sameSong(a, b) {
  return a && b && a.artist === b.artist && a.title === b.title;
}

function showEvery() {
  document.getElementById("fix-title-field").hidden = fixEvery.checked;
}

function renderFixes(settings) {
  const playing = settings.playing;
  const sent = settings.sent_as;
  const shown = `${playing?.artist}\n${playing?.title}\n${sent?.artist}\n${sent?.title}`;
  if (shown !== fixShown && !(fixEdited && sameSong(playing, fixSong))) {
    fixShown = shown;
    fixSong = playing;
    fixEdited = false;
    if (playing) {
      document.getElementById("fix-source").textContent = sent
        ? `The player sends ${playing.title} · ${playing.artist}, fixed to ${sent.title} · ${sent.artist}.`
        : `The player sends: ${playing.title} · ${playing.artist}`;
      document.getElementById("fix-every-label").textContent = `Fix every song by ${playing.artist}, not just this one`;
      fixArtist.value = (sent ?? playing).artist;
      fixTitle.value = (sent ?? playing).title;
      fixEvery.checked = false;
      showEvery();
    }
  }
  fixForm.hidden = !fixSong;
  document.getElementById("fix-idle").hidden = Boolean(fixSong);

  document.getElementById("fix-list").replaceChildren(
    ...settings.fixes.map((fix, index) => {
      const item = document.createElement("li");
      const text = document.createElement("span");
      text.textContent = `${fix.from} → ${fix.to}`;
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "secondary";
      remove.textContent = "Remove";
      remove.addEventListener("click", () => act(() => invoke("remove_fix", { index })));
      item.append(text, remove);
      return item;
    }),
  );
}

for (const input of [fixArtist, fixTitle, fixEvery]) {
  input.addEventListener("input", () => {
    fixEdited = true;
  });
}
fixEvery.addEventListener("change", showEvery);

fixForm.addEventListener("submit", (event) => {
  event.preventDefault();
  if (!fixSong) return;
  const song = fixSong;
  act(async () => {
    const settings = await invoke("add_fix", {
      artist: song.artist,
      title: song.title,
      newArtist: fixArtist.value,
      newTitle: fixTitle.value,
      everySong: fixEvery.checked,
    });
    fixEdited = false;
    fixShown = null;
    return settings;
  });
});

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

async function start() {
  await refresh();
  if (location.hash) document.getElementById(location.hash.slice(1))?.scrollIntoView();
}

start();
setInterval(refresh, 3000);
