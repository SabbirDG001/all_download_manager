"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => document.querySelectorAll(sel);

const state = {
  entries: new Map(), // id -> entry
  filter: "all",
  selected: new Set(),
  anchor: null, // last clicked id, for shift-click ranges
  settings: null,
};

// ---------- Formatting ----------

function fmtBytes(n) {
  if (n == null) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i === 0 ? 0 : n < 10 ? 2 : 1)} ${units[i]}`;
}

function fmtDuration(secs) {
  if (!isFinite(secs) || secs < 0) return "";
  secs = Math.round(secs);
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${String(secs % 60).padStart(2, "0")}s`;
  return `${Math.floor(secs / 3600)}h ${String(Math.floor((secs % 3600) / 60)).padStart(2, "0")}m`;
}

function percent(e) {
  if (e.status === "completed") return 100;
  if (!e.total) return null;
  return Math.min(100, (e.downloaded / e.total) * 100);
}

function displayName(e) {
  if (e.filename) return e.filename;
  try {
    const last = new URL(e.url).pathname.split("/").filter(Boolean).pop();
    return last ? decodeURIComponent(last) : e.url;
  } catch { return e.url; }
}

const KINDS = {
  video: ["mp4", "mkv", "webm", "avi", "mov", "flv", "wmv", "m4v", "ts"],
  audio: ["mp3", "m4a", "flac", "wav", "aac", "ogg", "opus", "wma"],
  archive: ["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso", "zst"],
  program: ["exe", "msi", "msix", "apk", "dmg", "deb", "appimage", "bat"],
  document: ["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "epub", "csv"],
  image: ["jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "heic"],
};

function extOf(name) {
  const m = /\.([a-z0-9]{1,5})$/i.exec(name);
  return m ? m[1].toLowerCase() : "";
}

function kindOf(ext) {
  for (const [kind, list] of Object.entries(KINDS)) if (list.includes(ext)) return kind;
  return "other";
}

function statusLabel(e) {
  switch (e.status) {
    case "queued": return "Queued";
    case "downloading": return e.total == null && e.downloaded === 0 ? "Connecting…" : "Downloading";
    case "paused": return "Paused";
    case "completed": return "Finished";
    case "failed": return "Failed";
  }
  return e.status;
}

// ---------- Filtering and ordering ----------

const FILTERS = {
  all: () => true,
  active: (e) => e.status === "downloading" || e.status === "queued",
  unfinished: (e) => e.status === "paused" || e.status === "failed",
  completed: (e) => e.status === "completed",
};

function visibleEntries() {
  return [...state.entries.values()]
    .filter(FILTERS[state.filter])
    .sort((a, b) => b.addedAt - a.addedAt || b.id - a.id);
}

// ---------- Rendering ----------

const rows = $("#rows");
const rowById = new Map();

function createRow(e) {
  const tr = document.createElement("tr");
  tr.dataset.id = e.id;
  tr.innerHTML = `
    <td><div class="name"><span class="ext"></span><span class="name-text"></span></div></td>
    <td class="num size"></td>
    <td><div class="progress"><div class="bar"><div></div></div><span class="pct"></span></div></td>
    <td class="num speed"></td>
    <td class="num eta"></td>
    <td><span class="status"><span class="dot"></span><span class="status-text"></span></span></td>`;
  return tr;
}

function updateRow(tr, e) {
  const name = displayName(e);
  const ext = extOf(name);
  tr.dataset.status = e.status;
  tr.classList.toggle("selected", state.selected.has(e.id));
  tr.title = e.error ? `${e.url}\n\n${e.error}` : e.url;

  const badge = tr.querySelector(".ext");
  badge.textContent = ext.slice(0, 4) || "file";
  badge.dataset.kind = kindOf(ext);
  tr.querySelector(".name-text").textContent = name;

  tr.querySelector(".size").textContent = e.total != null ? fmtBytes(e.total) : e.downloaded ? fmtBytes(e.downloaded) : "—";

  const pct = percent(e);
  const bar = tr.querySelector(".bar");
  const indeterminate = pct == null && e.status === "downloading";
  bar.classList.toggle("indeterminate", indeterminate);
  bar.firstElementChild.style.width = indeterminate ? "" : `${pct ?? 0}%`;
  tr.querySelector(".pct").textContent = pct == null ? "" : `${Math.floor(pct)}%`;

  const downloading = e.status === "downloading";
  tr.querySelector(".speed").textContent = downloading && e.speed > 0 ? `${fmtBytes(e.speed)}/s` : "";
  tr.querySelector(".eta").textContent =
    downloading && e.total && e.speed > 0 ? fmtDuration((e.total - e.downloaded) / e.speed) : "";
  const label = statusLabel(e);
  tr.querySelector(".status-text").textContent =
    downloading && e.connections > 1 ? `${label} · ${e.connections}` : label;
  tr.querySelector(".status").title =
    downloading && e.connections ? `${e.connections} connections`
    : e.status === "paused" && !e.resumable && e.downloaded > 0 ? "This server can't resume, so it will start over"
    : e.error || "";
}

/** Rebuilds the table in the current order, reusing existing rows. */
function renderList() {
  const list = visibleEntries();
  const keep = new Set(list.map((e) => e.id));
  for (const [id, tr] of rowById) {
    if (!keep.has(id)) { tr.remove(); rowById.delete(id); }
  }
  let prev = null;
  for (const e of list) {
    let tr = rowById.get(e.id);
    if (!tr) { tr = createRow(e); rowById.set(e.id, tr); }
    updateRow(tr, e);
    const expectedPos = prev ? prev.nextSibling : rows.firstChild;
    if (tr !== expectedPos) rows.insertBefore(tr, expectedPos);
    prev = tr;
  }
  $("#empty").hidden = list.length > 0;
  renderChrome();
}

/** Counts, status bar and button states. */
function renderChrome() {
  const all = [...state.entries.values()];
  for (const [name, fn] of Object.entries(FILTERS)) {
    const n = all.filter(fn).length;
    $(`[data-count="${name}"]`).textContent = n || "";
  }
  const downloading = all.filter((e) => e.status === "downloading");
  const queued = all.filter((e) => e.status === "queued").length;
  const speed = downloading.reduce((s, e) => s + (e.speed || 0), 0);
  const parts = [];
  if (downloading.length) parts.push(`${downloading.length} downloading`);
  if (queued) parts.push(`${queued} queued`);
  $("#status-text").textContent = parts.join(" · ") || `${all.length} download${all.length === 1 ? "" : "s"}`;
  $("#status-speed").textContent = speed > 0 ? `${fmtBytes(speed)}/s` : "";

  const sel = selectedEntries();
  $("#btn-resume").disabled = !sel.some(canResume);
  $("#btn-pause").disabled = !sel.some(canPause);
  $("#btn-delete").disabled = sel.length === 0;
  $("#btn-resume-all").disabled = !all.some(canResume);
  $("#btn-pause-all").disabled = !all.some(canPause);
  $("#btn-clear").disabled = !all.some((e) => e.status === "completed");
}

const canResume = (e) => e.status === "paused" || e.status === "failed";
const canPause = (e) => e.status === "downloading" || e.status === "queued";

let chromeQueued = false;
function scheduleChrome() {
  if (chromeQueued) return;
  chromeQueued = true;
  requestAnimationFrame(() => { chromeQueued = false; renderChrome(); });
}

function selectedEntries() {
  return [...state.selected].map((id) => state.entries.get(id)).filter(Boolean);
}

// ---------- Data from the backend ----------

function setAll(list) {
  state.entries = new Map(list.map((e) => [e.id, e]));
  for (const id of [...state.selected]) if (!state.entries.has(id)) state.selected.delete(id);
  renderList();
}

function updateOne(e) {
  const old = state.entries.get(e.id);
  state.entries.set(e.id, e);
  const tr = rowById.get(e.id);
  if (old && old.status !== e.status) { renderList(); return; }
  if (tr) updateRow(tr, e);
  scheduleChrome();
}

async function call(cmd, args) {
  try {
    return await invoke(cmd, args);
  } catch (err) {
    toast(String(err));
    throw err;
  }
}

// ---------- Selection ----------

rows.addEventListener("mousedown", (ev) => {
  const tr = ev.target.closest("tr");
  if (!tr) return;
  const id = Number(tr.dataset.id);
  if (ev.button === 2) {
    if (!state.selected.has(id)) { state.selected = new Set([id]); state.anchor = id; }
  } else if (ev.shiftKey && state.anchor != null) {
    const ids = visibleEntries().map((e) => e.id);
    const [a, b] = [ids.indexOf(state.anchor), ids.indexOf(id)].sort((x, y) => x - y);
    state.selected = new Set(ids.slice(a, b + 1));
  } else if (ev.ctrlKey || ev.metaKey) {
    state.selected.has(id) ? state.selected.delete(id) : state.selected.add(id);
    state.anchor = id;
  } else {
    state.selected = new Set([id]);
    state.anchor = id;
  }
  renderList();
});

$("#list").addEventListener("mousedown", (ev) => {
  if (!ev.target.closest("tr") && !ev.target.closest(".empty") && ev.button === 0) {
    state.selected.clear();
    renderList();
  }
});

rows.addEventListener("dblclick", (ev) => {
  const tr = ev.target.closest("tr");
  if (tr) openEntry(state.entries.get(Number(tr.dataset.id)));
});

function openEntry(e) {
  if (!e) return;
  if (e.status === "completed" && e.path) call("open_file", { path: e.path });
  else if (canResume(e)) call("resume", { ids: [e.id] });
}

// ---------- Toolbar ----------

const ids = (list) => list.map((e) => e.id);

$("#btn-add").onclick = () => openAdd();
$("#btn-resume").onclick = () => call("resume", { ids: ids(selectedEntries()) });
$("#btn-pause").onclick = () => call("pause", { ids: ids(selectedEntries()) });
$("#btn-delete").onclick = () => openDelete(selectedEntries());
$("#btn-resume-all").onclick = () => call("resume_all");
$("#btn-pause-all").onclick = () => call("pause_all");
$("#btn-clear").onclick = () => call("clear_completed");
$("#btn-settings").onclick = () => openSettings();

for (const btn of $$(".filter")) {
  btn.onclick = () => {
    state.filter = btn.dataset.filter;
    for (const b of $$(".filter")) b.classList.toggle("active", b === btn);
    renderList();
  };
}

// ---------- Dialogs ----------

for (const btn of $$("[data-close]")) btn.onclick = () => btn.closest("dialog").close();

for (const btn of $$("[data-browse]")) {
  btn.onclick = async () => {
    const input = document.getElementById(btn.dataset.browse);
    const folder = await call("pick_folder", { start: input.value || state.settings?.download_dir || null });
    if (folder) input.value = folder;
  };
}

function looksLikeUrl(text) {
  return /^https?:\/\/\S+$/i.test(text.trim());
}

function urlsFrom(text) {
  return text.split(/\s+/).map((s) => s.trim()).filter(looksLikeUrl);
}

async function openAdd(prefill = "") {
  if (!state.settings) await loadSettings();
  $("#add-urls").value = prefill;
  $("#add-dir").value = state.settings.download_dir;
  $("#add-name").value = "";
  $("#add-start").checked = true;
  updateAddName();
  $("#dlg-add").showModal();
  $("#add-urls").focus();
}

function updateAddName() {
  const several = urlsFrom($("#add-urls").value).length > 1;
  $("#add-name").disabled = several;
  if (several) $("#add-name").value = "";
}
$("#add-urls").addEventListener("input", updateAddName);

$("#form-add").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const urls = urlsFrom($("#add-urls").value);
  if (urls.length === 0) { toast("Paste a link that starts with http:// or https://"); return; }
  const dir = $("#add-dir").value.trim() || null;
  const filename = $("#add-name").value.trim() || null;
  const paused = !$("#add-start").checked;
  try {
    for (const url of urls) {
      await call("add", { request: { url, dir, filename: urls.length === 1 ? filename : null, paused } });
    }
    $("#dlg-add").close();
  } catch { /* toast already shown */ }
});

let pendingDelete = [];
function openDelete(list) {
  if (list.length === 0) return;
  pendingDelete = ids(list);
  const one = list.length === 1;
  $("#delete-title").textContent = one ? "Delete this download?" : `Delete ${list.length} downloads?`;
  $("#delete-text").textContent = one ? displayName(list[0]) : "They will be removed from the list.";
  $("#delete-files").checked = false;
  $("#dlg-delete").showModal();
}

$("#form-delete").addEventListener("submit", (ev) => {
  ev.preventDefault();
  call("remove", { ids: pendingDelete, deleteFiles: $("#delete-files").checked });
  $("#dlg-delete").close();
});

async function loadSettings() {
  const info = await call("get_settings");
  state.settings = info.settings;
  return info;
}

async function openSettings() {
  const info = await loadSettings();
  const s = info.settings;
  $("#set-download-dir").value = s.download_dir;
  $("#set-temp-dir").value = s.temp_dir || "";
  $("#set-connections").value = s.connections;
  $("#set-max-active").value = s.max_active;
  $("#set-user-agent").value = s.user_agent || "";
  $("#set-info").textContent = info.portable
    ? `Portable mode: settings and the download list are kept next to the program, in ${info.path}`
    : `Settings are saved in ${info.path}. For portable mode, put a settings.json file containing {} next to the program.`;
  $("#dlg-settings").showModal();
}

$("#set-temp-clear").onclick = () => { $("#set-temp-dir").value = ""; };

$("#form-settings").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const settings = {
    ...state.settings,
    download_dir: $("#set-download-dir").value.trim(),
    temp_dir: $("#set-temp-dir").value.trim() || null,
    connections: Number($("#set-connections").value),
    max_active: Number($("#set-max-active").value),
    user_agent: $("#set-user-agent").value.trim() || null,
  };
  try {
    await call("save_settings", { settings });
    state.settings = settings;
    $("#dlg-settings").close();
    toast("Settings saved");
  } catch { /* toast already shown */ }
});

// ---------- Context menu ----------

const menu = $("#menu");

document.addEventListener("contextmenu", (ev) => {
  if (ev.target.closest("input, textarea")) return;
  ev.preventDefault();
  const tr = ev.target.closest("#rows tr");
  if (!tr) { hideMenu(); return; }
  const sel = selectedEntries();
  const single = sel.length === 1 ? sel[0] : null;
  const done = single && single.status === "completed" && single.path;
  menu.querySelector('[data-action="open"]').disabled = !done;
  menu.querySelector('[data-action="folder"]').disabled = !done;
  menu.querySelector('[data-action="resume"]').disabled = !sel.some(canResume);
  menu.querySelector('[data-action="pause"]').disabled = !sel.some(canPause);
  menu.querySelector('[data-action="copy"]').disabled = !single;
  menu.hidden = false;
  const { innerWidth: w, innerHeight: h } = window;
  const r = menu.getBoundingClientRect();
  menu.style.left = `${Math.min(ev.clientX, w - r.width - 4)}px`;
  menu.style.top = `${Math.min(ev.clientY, h - r.height - 4)}px`;
});

function hideMenu() { menu.hidden = true; }
document.addEventListener("mousedown", (ev) => { if (!menu.contains(ev.target)) hideMenu(); });
window.addEventListener("blur", hideMenu);

menu.addEventListener("click", async (ev) => {
  const action = ev.target.closest("button")?.dataset.action;
  if (!action) return;
  hideMenu();
  const sel = selectedEntries();
  switch (action) {
    case "open": call("open_file", { path: sel[0].path }); break;
    case "folder": call("show_in_folder", { path: sel[0].path }); break;
    case "resume": call("resume", { ids: ids(sel) }); break;
    case "pause": call("pause", { ids: ids(sel) }); break;
    case "copy":
      try { await navigator.clipboard.writeText(sel[0].url); toast("Link copied"); }
      catch { toast("Couldn't copy the link"); }
      break;
    case "delete": openDelete(sel); break;
  }
});

// ---------- Keyboard, paste, drag and drop ----------

const dialogOpen = () => document.querySelector("dialog[open]") != null;

document.addEventListener("keydown", (ev) => {
  if (dialogOpen() || ev.target.closest("input, textarea")) return;
  const ctrl = ev.ctrlKey || ev.metaKey;
  if (ev.key === "Delete") { openDelete(selectedEntries()); ev.preventDefault(); }
  else if (ctrl && ev.key.toLowerCase() === "a") {
    state.selected = new Set(visibleEntries().map((e) => e.id));
    renderList();
    ev.preventDefault();
  } else if (ctrl && ev.key.toLowerCase() === "n") { openAdd(); ev.preventDefault(); }
  else if (ev.key === "Enter") { const sel = selectedEntries(); if (sel.length === 1) openEntry(sel[0]); }
  else if (ev.key === "Escape") { state.selected.clear(); renderList(); }
});

// Ctrl+V anywhere outside a text box opens "Add download" with the link.
document.addEventListener("paste", (ev) => {
  if (dialogOpen() || ev.target.closest("input, textarea")) return;
  const text = ev.clipboardData?.getData("text") || "";
  const urls = urlsFrom(text);
  if (urls.length) { ev.preventDefault(); openAdd(urls.join("\n")); }
});

// Drag a link from the browser into the window.
const listEl = $("#list");
document.addEventListener("dragover", (ev) => {
  ev.preventDefault();
  listEl.classList.add("drop-target");
});
document.addEventListener("dragleave", (ev) => {
  if (ev.relatedTarget == null) listEl.classList.remove("drop-target");
});
document.addEventListener("drop", (ev) => {
  ev.preventDefault();
  listEl.classList.remove("drop-target");
  const text = ev.dataTransfer?.getData("text/uri-list") || ev.dataTransfer?.getData("text/plain") || "";
  const urls = urlsFrom(text.replace(/^#.*$/gm, ""));
  if (urls.length && !dialogOpen()) openAdd(urls.join("\n"));
});

// ---------- Toast ----------

let toastTimer = null;
function toast(message) {
  const el = $("#toast");
  el.textContent = message;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.hidden = true; }, 3500);
}

// ---------- Start ----------

window.addEventListener("error", (ev) => toast(`Unexpected error: ${ev.message}`));
window.addEventListener("unhandledrejection", (ev) => toast(`Unexpected error: ${ev.reason}`));

(async () => {
  await listen("downloads", (ev) => {
    const change = ev.payload;
    if (change.kind === "list") setAll(change.data);
    else updateOne(change.data);
  });
  setAll(await call("list"));
  loadSettings();
})();
