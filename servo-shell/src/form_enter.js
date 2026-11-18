(() => {
  if (window.top !== window || window.soundscapeEnterReady) return;
  window.soundscapeEnterReady = true;
  const skipped = ["submit", "button", "checkbox", "radio", "file", "reset", "image", "range", "color"];
  addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || event.defaultPrevented || event.isComposing) return;
    if (event.ctrlKey || event.altKey || event.metaKey || event.shiftKey) return;
    const field = event.target;
    if (!(field instanceof HTMLInputElement) || !field.form) return;
    if (skipped.includes(field.type)) return;
    const form = field.form;
    if (form.querySelector('input[type="submit"], input[type="image"]')) return;
    const button = [...form.querySelectorAll("button")].find(
      (each) => (each.getAttribute("type") ?? "submit").toLowerCase() === "submit",
    );
    if (!button || button.disabled) return;
    event.preventDefault();
    button.click();
  });
})();
