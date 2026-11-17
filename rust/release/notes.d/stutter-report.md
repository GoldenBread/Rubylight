# Stutter report

- **See which stage took the time.** The console's **Stream details & performance** now says where a stream's new pictures lost time against the stream's own usual path, naming capture, conversion, encoding, packetizing or sending, and each stream's end writes a `stream stutter report` line to the log with the same figures. It measures the host's part only: the network, the decoder and the display come after the last packet, and a game slower than the stream shows as repeated pictures instead.
