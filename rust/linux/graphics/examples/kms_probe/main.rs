//! KMS capture probe: follows one output's page flips and, every few seconds, takes the shown
//! framebuffer and the cursor plane, imports the picture through VA-API as the host's capture
//! does (or maps a linear one without VA-API), and writes them as PNGs with the active session
//! in their names. Frames come from the KMS broker (`rubylight-kms.socket`; this user in the
//! `rubylight` group), or with `--direct` from the card itself, which needs `CAP_SYS_ADMIN`
//! (`setcap cap_sys_admin+ep` on the binary). Switch to the login screen meanwhile.
//!
//! `kms_probe [--direct] [--card /dev/dri/cardN] [--output DP-1] [--seconds 60] [--every 3]
//! [--out DIR]`
#[cfg(target_os = "linux")]
mod probe;

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    probe::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("kms_probe runs on Linux only");
}
