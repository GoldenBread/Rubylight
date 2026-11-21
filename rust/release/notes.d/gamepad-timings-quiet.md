# Logging
- **Quieter gamepad timing logs.** The 5-second `gamepad timings` line now logs at `INFO` only when controller input batches or feedback events occurred, moving idle zero-activity windows to `DEBUG`.
