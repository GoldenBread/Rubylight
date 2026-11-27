//! `rubylight-kms-broker serve`: one connection of the socket-activated `rubylight-kms@`
//! unit (the connection is stdin). See the library's header.
#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args != ["serve"] {
        eprintln!("usage: rubylight-kms-broker serve");
        return std::process::ExitCode::FAILURE;
    }
    match rubylight_kms_broker::serve::serve_stdio() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rubylight-kms-broker: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("rubylight-kms-broker runs on Linux only");
    std::process::exit(1);
}
