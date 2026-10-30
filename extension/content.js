// Shows a "Download" button over videos while the pointer is on them. On video
// sites (YouTube and others) it opens a menu of the qualities the video offers,
// like IDM's; elsewhere it sends the video straight to the app.

(() => {
  if (window.__admVideoButton) return;
  window.__admVideoButton = true;

  const MIN_W = 200;
  const MIN_H = 120;
  let enabled = true;
  let host = null;
  let root = null;
  let button = null;
  let menu = null;
  let current = null;
  let frame = 0;
  let menuOpen = false;
  const formatCache = new Map(); // page URL -> promise of the app's answer

  chrome.storage.local.get({ showVideoButton: true }, (o) => { enabled = o.showVideoButton; });
  chrome.storage.onChanged.addListener((changes) => {
    if (changes.showVideoButton) {
      enabled = changes.showVideoButton.newValue;
      if (!enabled) hide(true);
    }
  });

  const onVideoSite = () => typeof isMediaSite === "function" && isMediaSite(location.href);

  function fmtBytes(n) {
    if (!n) return "";
    const units = ["B", "KB", "MB", "GB"];
    let i = 0;
    while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
    return `${n.toFixed(i < 2 ? 0 : 1)} ${units[i]}`;
  }

  function ensureButton() {
    if (host) return;
    host = document.createElement("div");
    host.style.cssText = "position:fixed;top:0;left:0;z-index:2147483647;display:none;";
    root = host.attachShadow({ mode: "closed" });
    root.innerHTML = `
      <style>
        * { box-sizing: border-box; font: 12px/1.35 "Segoe UI", system-ui, sans-serif; }
        .btn {
          all: initial; display: flex; align-items: center; gap: 6px; cursor: pointer;
          padding: 6px 11px 6px 9px; border-radius: 6px; margin-left: auto; width: max-content;
          background: rgba(37, 99, 235, .96); color: #fff;
          font: 600 12px/1 "Segoe UI", system-ui, sans-serif; box-shadow: 0 2px 10px rgba(0, 0, 0, .35);
        }
        .btn:hover { background: #1d4ed8; }
        svg { width: 14px; height: 14px; fill: none; stroke: currentColor; stroke-width: 2.4; stroke-linecap: round; stroke-linejoin: round; }
        .menu {
          margin-top: 6px; width: 340px; max-height: 340px; overflow: auto; border-radius: 8px;
          background: #1f2227; color: #e7e9ec; box-shadow: 0 8px 28px rgba(0, 0, 0, .5); padding: 4px;
        }
        .menu[hidden] { display: none; }
        .head { padding: 6px 10px 4px; color: #9aa1ab; font-size: 11px; text-transform: uppercase; letter-spacing: .04em; }
        .item { display: flex; gap: 8px; align-items: center; padding: 6px 10px; border-radius: 5px; cursor: pointer; }
        .item:hover { background: #2f3540; }
        .item b { font-weight: 600; min-width: 80px; }
        .item .d { color: #9aa1ab; flex: 1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
        .item .s { color: #9aa1ab; font-variant-numeric: tabular-nums; }
        .note { padding: 10px; color: #9aa1ab; }
        .sep { height: 1px; background: #33373e; margin: 4px 6px; }
      </style>
      <button type="button" class="btn" title="Download with All Download Manager">
        <svg viewBox="0 0 24 24"><path d="M12 4v11M7 10l5 5 5-5M5 20h14"/></svg><span>Download</span>
      </button>
      <div class="menu" hidden></div>`;
    button = root.querySelector(".btn");
    menu = root.querySelector(".menu");
    button.addEventListener("click", onClick, true);
    (document.body || document.documentElement).appendChild(host);
  }

  function label(text) {
    button.querySelector("span").textContent = text;
  }

  function send(format) {
    const video = current;
    closeMenu();
    label("Sending…");
    chrome.runtime.sendMessage(
      {
        type: "grab-video",
        src: video ? video.currentSrc || video.src || "" : "",
        title: document.title,
        pageUrl: location.href,
        format,
      },
      (res) => {
        label(res?.ok ? "Sent ✓" : "Failed");
        setTimeout(() => label("Download"), 1800);
      },
    );
  }

  function item(text, detail, size, onPick) {
    const el = document.createElement("div");
    el.className = "item";
    const b = document.createElement("b");
    b.textContent = text;
    const d = document.createElement("span");
    d.className = "d";
    d.textContent = detail || "";
    const s = document.createElement("span");
    s.className = "s";
    s.textContent = size ? `~${fmtBytes(size)}` : "";
    el.append(b, d, s);
    el.addEventListener("click", (ev) => { ev.stopPropagation(); onPick(); });
    return el;
  }

  function note(text) {
    const el = document.createElement("div");
    el.className = "note";
    el.textContent = text;
    return el;
  }

  function heading(text) {
    const el = document.createElement("div");
    el.className = "head";
    el.textContent = text;
    return el;
  }

  async function openMenu() {
    menuOpen = true;
    menu.hidden = false;
    menu.replaceChildren(note("Getting the qualities…"));
    const url = location.href;
    if (!formatCache.has(url)) {
      formatCache.set(url, new Promise((resolve) => chrome.runtime.sendMessage({ type: "formats", url }, resolve)));
    }
    const res = await formatCache.get(url);
    if (!menuOpen) return;
    if (!res?.ok) {
      formatCache.delete(url);
      menu.replaceChildren(note(res?.error || "The app didn't answer."), item("Choose in the app…", "", null, () => send(null)));
      return;
    }
    const info = res.info;
    const items = [];
    if (info.video.length) {
      items.push(heading("Video"));
      for (const o of info.video) items.push(item(o.label, o.detail, o.size, () => send(o.format)));
    }
    if (info.audio.length) {
      items.push(heading("Audio only"));
      for (const o of info.audio) items.push(item(o.label, o.detail, o.size, () => send(o.format)));
    }
    const sep = document.createElement("div");
    sep.className = "sep";
    items.push(sep, item("Choose in the app…", "", null, () => send(null)));
    menu.replaceChildren(...items);
  }

  function closeMenu() {
    menuOpen = false;
    if (menu) menu.hidden = true;
  }

  function onClick(ev) {
    ev.preventDefault();
    ev.stopPropagation();
    if (!current) return;
    if (!onVideoSite()) { send(null); return; }
    if (menuOpen) closeMenu(); else openMenu();
  }

  function videoAt(x, y) {
    for (const v of document.querySelectorAll("video")) {
      const r = v.getBoundingClientRect();
      if (r.width >= MIN_W && r.height >= MIN_H && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) {
        return v;
      }
    }
    return null;
  }

  function show(video) {
    ensureButton();
    current = video;
    const r = video.getBoundingClientRect();
    host.style.display = "block";
    const w = menuOpen ? 340 : host.offsetWidth || 110;
    host.style.left = `${Math.max(4, r.right - w - 10)}px`;
    host.style.top = `${Math.max(4, r.top + 10)}px`;
  }

  function hide(force) {
    if (menuOpen && !force) return;
    closeMenu();
    if (host) host.style.display = "none";
    current = null;
  }

  document.addEventListener(
    "mousemove",
    (ev) => {
      if (!enabled || frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        if (host && ev.composedPath().includes(host)) return;
        const video = videoAt(ev.clientX, ev.clientY);
        if (video) show(video);
        else hide(false);
      });
    },
    { passive: true },
  );
  // Clicking elsewhere closes the menu.
  document.addEventListener("mousedown", (ev) => {
    if (menuOpen && host && !ev.composedPath().includes(host)) hide(true);
  }, true);
  document.addEventListener("scroll", () => hide(true), { passive: true, capture: true });
})();
