"use strict";

// The small "Download complete" window, opened by the app for each finished download.

const { invoke } = window.__TAURI__.core;
const $ = (sel) => document.querySelector(sel);

function fmtBytes(n) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

const close = () => invoke("close_this_window");

(async () => {
  const id = Number(new URLSearchParams(location.search).get("id"));
  const entry = await invoke("get_entry", { id });
  if (!entry || !entry.path) { close(); return; }
  $("#done-name").textContent = entry.filename || entry.title || "Download complete";
  $("#done-info").textContent = [entry.total ? fmtBytes(entry.total) : "", entry.formatLabel || ""].filter(Boolean).join(" · ");
  $("#done-path").textContent = entry.path;
  $("#done-open").onclick = async () => { await invoke("open_file", { path: entry.path }).catch(() => {}); close(); };
  $("#done-folder").onclick = async () => { await invoke("show_in_folder", { path: entry.path }).catch(() => {}); close(); };
  $("#done-close").onclick = close;
  $("#done-open").focus();
})();

document.addEventListener("keydown", (ev) => { if (ev.key === "Escape") close(); });
document.addEventListener("contextmenu", (ev) => ev.preventDefault());
