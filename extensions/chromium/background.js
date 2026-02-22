const HOST = "com.loki.dm";

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: "loki-download-link",
    title: "Download with Loki DM",
    contexts: ["link", "video", "audio"]
  });
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const targetUrl = info.linkUrl || info.srcUrl || tab?.url;
  if (!targetUrl) {
    return;
  }

  await sendToLoki({ action: "queue", url: targetUrl });
});

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === "LOKI_DM_DOWNLOAD" && message.url) {
    sendToLoki({ action: "queue", url: message.url })
      .then((response) => sendResponse({ ok: true, response }))
      .catch((error) => sendResponse({ ok: false, error: String(error) }));
    return true;
  }

  return false;
});

chrome.downloads.onCreated.addListener((item) => {
  chrome.storage.sync.get({ interceptAll: false }, (cfg) => {
    if (!cfg.interceptAll) {
      return;
    }

    sendToLoki({ action: "queue", url: item.finalUrl || item.url }).catch(() => {
      // Native host can be unavailable while extension remains installed.
    });
  });
});

function sendToLoki(payload) {
  return new Promise((resolve, reject) => {
    chrome.runtime.sendNativeMessage(HOST, payload, (response) => {
      const err = chrome.runtime.lastError;
      if (err) {
        reject(err.message);
        return;
      }
      resolve(response);
    });
  });
}
