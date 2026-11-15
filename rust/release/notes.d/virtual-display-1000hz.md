# Virtual display refresh

- **1000 Hz by default.** New virtual displays run at 1000 Hz, so a game capped at the stream's frame rate keeps its frames evenly spaced. With a 60 fps cap, 0.04 % of frame intervals were more than 4 ms off at 1000 Hz, against 0.23 % at twice and 2.22 % at four times the stream rate, where frames land in alternate refresh slots as the cap's timer drifts against the display's. Twice and four times the stream rate stay selectable, and Off still ends the automatic FPS cap.
- **A refused refresh no longer leaves the display slow.** When Windows refuses the refresh a virtual display asks for and keeps a slower one at the right resolution, the display now asks for the fastest refresh it offers below the refused one.
