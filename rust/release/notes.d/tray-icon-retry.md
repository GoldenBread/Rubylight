# Tray icon
- **Fewer tray icon warnings at startup.** Temporary Windows refusals while waiting for the taskbar or Explorer to settle are logged at debug level instead of warning, 3-second retries stop after a minute if the taskbar still refuses, and a duplicate taskbar notification after the icon is already shown no longer triggers a retry warning.
