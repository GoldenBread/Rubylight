## en
- A video decoder that is briefly slow to take frames no longer holds back the incoming stream, so a decoder hiccup no longer reads as network trouble. If it falls further behind, frames are dropped and a new key frame is requested, counted as decoder drops.
