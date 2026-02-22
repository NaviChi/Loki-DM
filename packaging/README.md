# Packaging Assets

This directory contains packaging metadata/templates used by release automation and manual packaging.

- `windows/loki-dm.wxs`: WiX installer definition for `loki-dm.exe` and `loki-dm-gui.exe`.
- `linux/loki-dm.desktop`: desktop launcher metadata.
- `linux/loki-dm.appdata.xml`: AppStream metadata.
- `macos/Info.plist`: macOS app bundle metadata template.

## Manual Packaging

### Windows MSI (WiX)

```powershell
candle.exe packaging\windows\loki-dm.wxs
light.exe -ext WixUIExtension -o loki-dm-windows.msi loki-dm.wixobj
```

### Tauri native bundles (preferred)

```bash
cd crates/loki-dm-gui
cargo tauri build --bundles app,appimage,deb,rpm,dmg,msi
```
