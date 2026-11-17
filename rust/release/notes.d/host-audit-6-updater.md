# Updates and settings
- **Turning automatic updates off removes a half-finished download** from the install folder at the next housekeeping pass, instead of leaving it there until the host restarts.
- **A settings save that fails no longer changes the SteamGridDB key.** The key is stored only after the settings file has been written, and when the key itself cannot be stored or removed the other settings still apply and the console says what happened to the key.
