# Changelog

Entries here are **user-facing**: the section matching the released version is
what the branded update window shows every user under "What's new". Keep bullets
short, benefit-first, and free of internal jargon (English — the update window's
localized fallback covers builds released without an entry).

The release workflow's `manifest` job reads the `## <version>` section verbatim;
no entry → users see the generic "Improvements and fixes." line and CI emits a
warning. Adding the entry is part of the version-bump checklist in the README.

## 1.4.3

- Enter payment details and see confirmation inside Orcaa, in Arabic or English.
- Resume an unfinished checkout without creating the purchase again.
- Return to Orcaa safely after required bank verification.

## 1.4.2

- Orcaa now runs on Windows 7. A dedicated build for Windows 7 SP1 counter PCs
  (64-bit and 32-bit) with everything the register needs: silent receipt,
  kitchen and label printing, the cash drawer, kiosk mode and automatic
  updates. Notifications there appear inside Orcaa and flash the taskbar
  instead of Windows toasts. Download it from orcaa.cloud/downloads.
- Nothing changes on Windows 10 and 11, macOS or Linux.

## 1.3.4

- One Orcaa app for everyone. The separate Orcaa Admin app is retired — sign in
  as you always do and Orcaa opens on the right workspace for your account.
- Summon Orcaa from anywhere with Ctrl+Shift+O, whichever workspace you land in.

## 1.2.0

- POS Station: instant receipt printing with no print dialog on thermal
  printers (network or serial), automatic cash-drawer opening on cash sales,
  and a printer test page — set it up in Point of Sale settings.
- Kiosk mode for counter PCs: launch with --kiosk and the app starts
  fullscreen, ready for the register.
- A one-time offer to start Orcaa with your computer, so notifications reach
  you all day.
- A branded Orcaa titlebar: the app's own top bar now doubles as the window
  titlebar on Windows, with the window buttons built in — no more generic
  system frame. On Mac the familiar traffic lights stay.
- Unread badge on the taskbar (Windows) and dock (Mac): see at a glance that
  something needs you, even after a toast has gone.
- Summon Orcaa from anywhere with Ctrl+Shift+O (admin app: Ctrl+Shift+A).
- Tray quick actions: jump straight to Point of Sale or your Dashboard from
  the tray icon's menu.
- Update prompts now show real release notes instead of a generic line.
- Security hardening of the app's built-in pages.
- Signed Windows installer (no more SmartScreen warning) once Azure Trusted
  Signing secrets are configured.

## 1.1.6

- Improvements and fixes.
