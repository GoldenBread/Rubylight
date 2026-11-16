# Session history

- **What each stream did, and what runs now.** **Advanced → Sessions** in the console shows the streams running now and the last 200 that ended, with the device, game, codec, resolution, average and p95 frame rate, bitrate, the loss the device reported, how long each ran and why it ended, in the host's words. The history is kept in the host's data directory for 90 days and drops the oldest sessions by itself, and the live half refreshes every 2 seconds without the stream card's polling. `GET /api/sessions/history` lists it for scripts and a Home dashboard.
