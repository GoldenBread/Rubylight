## desktop
- Fixed Windows Balanced V-Sync dropping to half frame rate when the swap chain was still busy right after vblank or under GPU load: no-wait presents now retry briefly through the post-vblank retirement window instead of sleeping a full refresh, and fall back to waiting presents when refusals continue past a whole refresh.
