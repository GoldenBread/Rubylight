//! KMS capture through a root broker. Reading the framebuffer another program (a compositor,
//! the login manager's) shows takes `CAP_SYS_ADMIN`, which the host should not hold: the
//! broker (`rubylight-kms-broker serve`, one per connection of a socket-activated unit) holds
//! it, follows one output's page flips and passes each new framebuffer's DMA-BUFs, and the
//! cursor plane's, to the host over a sequenced-packet socket. Only root and the `rubylight`
//! group may connect: whoever can, sees the screen, the login screen included.
//!
//! [`drm`] reads the card; [`protocol`] is the messages; [`client`] is the host's side.
#[cfg(target_os = "linux")]
pub mod client;
#[cfg(target_os = "linux")]
pub mod drm;
#[cfg(target_os = "linux")]
pub mod fdpass;
pub mod protocol;
#[cfg(target_os = "linux")]
pub mod serve;
