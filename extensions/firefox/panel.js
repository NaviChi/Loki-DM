const urlInput = document.getElementById("url");
const sendButton = document.getElementById("send");
const interceptInput = document.getElementById("interceptAll");

chrome.storage.sync.get({ interceptAll: false }, (cfg) => {
  interceptInput.checked = cfg.interceptAll;
});

interceptInput.addEventListener("change", () => {
  chrome.storage.sync.set({ interceptAll: interceptInput.checked });
});

sendButton.addEventListener("click", () => {
  const url = urlInput.value.trim();
  if (!url) {
    return;
  }

  chrome.runtime.sendMessage({ type: "LOKI_DM_DOWNLOAD", url }, () => {
    window.close();
  });
});
