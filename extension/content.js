// Shows a "Download" button over videos while the pointer is on them.

(() => {
  if (window.__admVideoButton) return;
  window.__admVideoButton = true;

  const MIN_W = 200;
  const MIN_H = 120;
  let enabled = true;
  let host = null;
  let button = null;
  let current = null;
  let frame = 0;

  chrome.storage.local.get({ showVideoButton: true }, (o) => { enabled = o.showVideoButton; });
  chrome.storage.onChanged.addListener((changes) => {
    if (changes.showVideoButton) {
      enabled = changes.showVideoButton.newValue;
      if (!enabled) hide();
    }
  });

  function ensureButton() {
    if (host) return;
    host = document.createElement("div");
    host.style.cssText = "position:fixed;top:0;left:0;z-index:2147483647;display:none;";
    const root = host.attachShadow({ mode: "closed" });
    root.innerHTML = `
      <style>
        button {
          all: initial; display: flex; align-items: center; gap: 6px; cursor: pointer;
          padding: 6px 11px 6px 9px; border-radius: 6px;
          background: rgba(37, 99, 235, .95); color: #fff;
          font: 600 12px/1 "Segoe UI", system-ui, sans-serif;
          box-shadow: 0 2px 10px rgba(0, 0, 0, .35);
        }
        button:hover { background: #1d4ed8; }
        svg { width: 14px; height: 14px; fill: none; stroke: currentColor; stroke-width: 2.4; stroke-linecap: round; stroke-linejoin: round; }
      </style>
      <button type="button" title="Download with All Download Manager">
        <svg viewBox="0 0 24 24"><path d="M12 4v11M7 10l5 5 5-5M5 20h14"/></svg><span>Download</span>
      </button>`;
    button = root.querySelector("button");
    button.addEventListener("click", onClick, true);
    (document.body || document.documentElement).appendChild(host);
  }

  function onClick(ev) {
    ev.preventDefault();
    ev.stopPropagation();
    if (!current) return;
    const label = button.querySelector("span");
    label.textContent = "Sending…";
    chrome.runtime.sendMessage(
      { type: "grab-video", src: current.currentSrc || current.src || "", title: document.title, pageUrl: location.href },
      (res) => {
        label.textContent = res?.ok ? "Sent ✓" : "Failed";
        setTimeout(() => { label.textContent = "Download"; }, 1800);
      },
    );
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
    const w = host.offsetWidth || 110;
    host.style.left = `${Math.max(4, r.right - w - 10)}px`;
    host.style.top = `${Math.max(4, r.top + 10)}px`;
  }

  function hide() {
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
        else hide();
      });
    },
    { passive: true },
  );
  document.addEventListener("scroll", hide, { passive: true, capture: true });
})();
