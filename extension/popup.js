const $ = (id) => document.getElementById(id);

function setStatus(kind, text) {
  $("status").className = `status ${kind}`;
  $("status-text").textContent = text;
}

async function checkApp() {
  const res = await chrome.runtime.sendMessage({ type: "ping" });
  if (res?.ok && res.reply?.running) {
    setStatus("ok", `Connected to the app (version ${res.reply.version})`);
  } else if (res?.ok) {
    setStatus("idle", "App installed; it starts when needed");
  } else {
    setStatus("missing", "App not found. Install it and open it once.");
    $("open").disabled = true;
  }
}

(async () => {
  const { catchDownloads } = await chrome.storage.local.get({ catchDownloads: true });
  $("catch").checked = catchDownloads;
  $("catch").addEventListener("change", (e) => chrome.storage.local.set({ catchDownloads: e.target.checked }));

  const { lastError } = (await chrome.storage.session?.get({ lastError: "" })) || {};
  if (lastError) {
    $("error").textContent = `Last problem: ${lastError}`;
    $("error").hidden = false;
  }
  chrome.runtime.sendMessage({ type: "clear-error" });

  $("open").addEventListener("click", async () => {
    $("open").disabled = true;
    const res = await chrome.runtime.sendMessage({ type: "show" });
    if (res?.ok) window.close();
    else {
      $("error").textContent = res?.error || "Couldn't open the app";
      $("error").hidden = false;
      $("open").disabled = false;
    }
  });

  checkApp();
})();
