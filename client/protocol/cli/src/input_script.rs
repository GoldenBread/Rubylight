//! `NIGHTJAR_CLI_INPUT_SCRIPT`: that many seconds after the first decoded picture, one of each
//! kind of input a host's injection test reads back: the A key, a left click, a relative move,
//! a scroll click up, and an Xbox controller arriving with A held and its left stick pushed
//! right, then released. A line `INPUT_SCRIPT sent` follows the last of them.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use nightjar_core::session::InputHandle;

use crate::harness::State;

// Moonlight's values: VK_A, the left button, LI_CTYPE_XBOX and A_FLAG.
const VK_A: u16 = 0x41;
const LEFT: u8 = 1;
const XBOX: u8 = 0x01;
const A_BUTTON: u32 = 0x1000;

/// Starts the script on a thread of its own when the variable names a delay of 0 to 600
/// seconds. The thread ends by itself; the stream does not wait for it.
pub fn start(input: InputHandle, state: Arc<State>) {
    let Some(delay) = std::env::var("NIGHTJAR_CLI_INPUT_SCRIPT")
        .ok()
        .and_then(|delay| delay.parse::<f64>().ok())
        .filter(|delay| (0.0..=600.0).contains(delay))
    else {
        return;
    };
    std::thread::spawn(move || {
        while state.counters.decoded_frames.load(Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_secs_f64(delay));
        let pause = || std::thread::sleep(Duration::from_millis(100));
        for down in [true, false] {
            input.input(|q| q.send_keyboard(VK_A, down, 0, 0).ok());
            pause();
        }
        for down in [true, false] {
            input.input(|q| q.send_mouse_button(LEFT, down).ok());
            pause();
        }
        input.input(|q| q.send_mouse_move(10, 5).ok());
        pause();
        input.input(|q| q.send_scroll(1).ok());
        pause();
        input.input(|q| q.send_controller_arrival(0, 1, XBOX, 0xFFFF, 0).ok());
        pause();
        input.input(|q| {
            q.send_controller(0, 1, A_BUTTON, 0, 0, (16000, 0), (0, 0))
                .ok()
        });
        std::thread::sleep(Duration::from_millis(500));
        input.input(|q| q.send_controller(0, 1, 0, 0, 0, (0, 0), (0, 0)).ok());
        println!(
            "INPUT_SCRIPT sent: key A, left click, move 10,5, scroll up, pad A and left stick"
        );
    });
}
