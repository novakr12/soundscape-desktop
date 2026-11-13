const { invoke } = window.__TAURI__.core;

const form = document.getElementById("connect");
const address = document.getElementById("address");
const message = document.getElementById("message");
const button = form.querySelector("button");

invoke("current_server")
  .then((server) => {
    if (server && !address.value) address.value = server.replace(/\/$/, "");
  })
  .catch(() => {});

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  message.hidden = true;
  button.disabled = true;
  button.textContent = "Checking…";
  try {
    await invoke("connect_server", { address: address.value });
  } catch (error) {
    message.textContent = String(error);
    message.hidden = false;
    button.disabled = false;
    button.textContent = "Connect";
    address.focus();
  }
});
