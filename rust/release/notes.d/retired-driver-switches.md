# Driver switches
- **Three driver switches are gone.** The compute queue that copies and converts captured frames, and PyroWave's encode queue, always run at high priority; the real-time option that could hang the graphics driver is removed. The AMD encoder always uses the driver's own input queue size and low-latency mode.
