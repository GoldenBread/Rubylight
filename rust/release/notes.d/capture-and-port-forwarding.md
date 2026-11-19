# Capture and port forwarding fixes
- **An app's window streams with AMD Direct Capture set.** A stream of the app's window now captures it with Windows Graphics Capture and says so, as it already did with Desktop Duplication, instead of failing to open.
- **Port forwarding recovers after the router or this PC's address changed.** After a new router, a router back at another address or a new address for this PC, the host looks for the router again by itself and tries PCP and NAT-PMP when UPnP is gone, instead of failing until the host restarts.
