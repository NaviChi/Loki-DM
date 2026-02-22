function injectFloatingPanel() {
  if (document.getElementById("loki-dm-float")) {
    return;
  }

  const videos = Array.from(document.querySelectorAll("video[src], source[src]")).slice(0, 1);
  if (videos.length === 0) {
    return;
  }

  const src = videos[0].src || videos[0].getAttribute("src");
  if (!src) {
    return;
  }

  const panel = document.createElement("button");
  panel.id = "loki-dm-float";
  panel.textContent = "Download Video (Loki DM)";
  panel.style.cssText = [
    "position: fixed",
    "right: 18px",
    "bottom: 18px",
    "z-index: 2147483647",
    "padding: 10px 14px",
    "border: none",
    "border-radius: 999px",
    "font: 600 13px system-ui, sans-serif",
    "color: #f5f7fb",
    "background: linear-gradient(135deg,#1f3d7a,#15a4c8)",
    "box-shadow: 0 8px 24px rgba(0,0,0,.35)",
    "cursor: pointer"
  ].join(";");

  panel.addEventListener("click", () => {
    chrome.runtime.sendMessage({
      type: "LOKI_DM_DOWNLOAD",
      url: src
    });
  });

  document.body.appendChild(panel);
}

setTimeout(injectFloatingPanel, 1200);
