# Logs and capture
- **The host's log comes back after a rotation that could not reopen it.** When the log had been moved aside but the new file could not be opened, writing went on in the moved file and the log itself stayed missing; the next line now opens it again. No line is lost.
- **Real-time GPU priority for the compute queue also applies to Windows Graphics Capture.** With capture through its helper process, the `compute_queue_realtime` setting did nothing; it now applies there as it does elsewhere. Nothing changes while it is off.
