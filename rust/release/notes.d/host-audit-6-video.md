# Video after jumbo streams
- **A stock Moonlight client gets video after a jumbo-packet client has streamed.** The video socket's don't-fragment setting now follows the live jumbo sessions: it is set when the first one starts and cleared when the last one ends, instead of whichever stream started last deciding it for every stream.
