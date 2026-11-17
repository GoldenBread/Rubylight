# Shutdown
- **A console launch is cancelled when the host service stops**, as a device's launch already was, so no game outlives the host and its preparation commands are undone.
- **A game that ignores close no longer keeps the stream's display and audio route at shutdown.** They go back before the game is asked to close, within the shutdown's own time.
