# AerOS goals

The feature list AerOS is working through (from the November 2026 wishlist),
ticked off as items land. "Partly" items say what is still missing.

## Debian-powered apps (through the built-in Linux guest)

- [x] Linux apps as real AerOS windows (own buffers, near-native drawing)
- [x] App Store installing Debian apps (installs live in RAM for now)
- [x] Windows programs (Bottles, built on Wine, in the App Store)
- [x] File converters in the App Store (LibreOffice, Switcheroo for pictures, HandBrake for video)
- [x] Text recognition from pictures (Frog in the App Store; a built-in Photos app is still to come)
- [x] QR code maker and scanner (Decoder in the App Store)
- [x] Compress, extract and browse archives (File Roller in the App Store)
- [x] Git client (gitg in the App Store)
- [x] Creative suite in the Store (GIMP, Inkscape, Kdenlive, Krita, Blender)
- [x] Offline live captions (Live Captions in the App Store)
- [x] Local AI assistant (Alpaca in the App Store; runs models on your own computer)

## Privacy

- [ ] Encrypted vault folders
- [ ] Password-protected folders
- [x] Hidden folders and files (Files: Ctrl+H shows, Ctrl+Shift+H hides)
- [ ] Hidden apps
- [ ] Duress PIN
- [ ] Second desktop (separate user space)
- [x] Encrypted password manager (KeePassXC in the App Store)
- [ ] USB port lockdown when Shield finds a virus

## Quality of life

- [x] Do Not Disturb (quick settings)
- [ ] Focus modes (beyond on/off Do Not Disturb)
- [x] Screen time with a daily limit (Settings > Power)
- [ ] Storage cleaner
- [x] Styleable terminal (6 themes: `theme`, or Spotlight "terminal theme")
- [x] Picture-in-picture for Linux windows (Super+P)
- [ ] Drag-and-drop shelf
- [ ] Spellcheck
- [x] Better Settings app (animated pages, Wi-Fi list, quick access from Spotlight)
- [x] Game mode (Super+G: crosshair, Do Not Disturb, Shift+< locks Shift)
- [x] Spotlight command bar (calculator, actions, settings, apps, web)
- [x] Keyboard shortcuts (Super+D/E/I/B/T/A/N/S, Ctrl+W/Q, Alt+Tab, Alt+F4...)
- [x] Fluid animations across the shell (springs, cascades, hover)

## Hardware

- [x] Battery health shown in Settings
- [x] Battery health warning below 50%
- [x] Hardware diagnostics (Settings > About > Hardware)
- [ ] Smart USB device recognition
- [ ] Sound tuned to the detected device
- [x] Slow-charge detection ("smart power meter": warns when charging power stays low)
- [x] Smart CPU core use (Linux guest gets one core per free CPU, up to 16)
- [x] Live wallpapers on high-resolution screens (rendered on a spare core)

## Big projects

- [ ] Wi-Fi on the laptop's MT7961 (scan returns no networks yet)
- [ ] Bluetooth
- [x] GPU: Radeon display driver through the Linux driver adapter (acceleration still to come)
- [x] Touchscreen driver (I2C-HID; needs testing on the laptop)
- [ ] Precision touchpad over I2C-HID (the touchpad uses the PS/2 path today)
- [ ] Phone as webcam, iPhone clipboard
- [ ] `.aer` app package format
- [ ] USB installer writer (Rufus-style)
- [ ] Self-healing system files
- [ ] Full Linux driver compatibility (the Linux guest covers apps meanwhile)
