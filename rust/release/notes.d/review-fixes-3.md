# Host fixes from a code review
- **Quit and End session work while a game is still launching.** They stop the launch instead of waiting behind it, for minutes if a preparation command hangs. That command is ended, and the undo commands of the ones that had finished still run.
- **A failed launch says which app's preparation command failed and what to do next.**
- **Update downloads resume.** A paused or broken-off download goes on where it stopped, one that receives nothing for 60 seconds ends, and an automatic installation that fails shows a warning in the console.
- **A device is told why its stream was refused.** A codec this PC cannot encode, or a stream that had to be encrypted and was not, leaves its reason for the device as a failed launch does.
- **Resuming at another frame rate keeps the game on its display.** The virtual display is retimed, not replaced, the resume takes the frame limiter and HDR profile set since the launch, and a display layout Windows refuses after the PC sat idle is tried again as on a fresh launch.
- **Keyboard and mouse keep working while the controllers reconnect.**
- **Adding a remote monitor no longer holds up the app list and other devices** while its display is made.
- **Stopping the Rubylight service puts Radeon settings and game files back before it closes the game**, so they are restored within the time Windows gives a service to stop. A launch still preparing, or one that starts while the host stops, is stopped too.
- After Continue here, a game's per-device settings file follows the device now playing. Compute conversion steps aside only after two encoder failures within a minute and is tried again after 5 quiet minutes, where one bad moment used to cost the rest of the session.
