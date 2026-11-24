//! `NIGHTJAR_CLI_PACKED_444=1`: the stream asks for packed full chroma
//! (`rubylight_protocol::packed444`) in every mode FFmpeg decodes, and the host picks one it
//! offers. The decoded pictures are then the coded ones, twice the padded height, whose size the
//! CLI checks and which `BUTTERPOLLO_TEST_FRAME_DUMP` writes for a test to take apart.
use nightjar_core::sdp::StreamConfig;
use rubylight_protocol::packed444::{ALL_MODES, Geometry};

/// Asks `stream` for packed full chroma when the variable says so; the size of the pictures
/// the decoder gives.
pub fn request(stream: &mut StreamConfig) -> (i32, i32) {
    let wanted = std::env::var("NIGHTJAR_CLI_PACKED_444").is_ok_and(|value| value == "1");
    let geometry = Geometry::new(stream.width as u32, stream.height as u32).filter(|_| wanted);
    let Some(geometry) = geometry else {
        return (stream.width, stream.height);
    };
    stream.experimental_packed_444 = true;
    stream.packed444_modes = ALL_MODES;
    let (width, height) = geometry.coded_size();
    println!(
        "PACKED444 requested picture={}x{} coded={width}x{height}",
        stream.width, stream.height
    );
    (width as i32, height as i32)
}
