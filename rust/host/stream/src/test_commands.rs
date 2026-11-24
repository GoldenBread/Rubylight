//! Programs and commands that run for a while, for tests, in the syntax of the OS's shell.

/// A shell command (an undo command) that takes about `seconds` seconds.
pub fn wait(seconds: u32) -> String {
    if cfg!(windows) {
        format!("ping -n {seconds} 127.0.0.1 >nul")
    } else {
        format!("sleep {seconds}")
    }
}

/// A program and its arguments that run for 30 seconds.
pub fn background_program() -> (String, Vec<String>) {
    if cfg!(windows) {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let program = format!(r"{root}\System32\PING.EXE");
        let args = ["-n", "30", "127.0.0.1"].map(String::from).to_vec();
        (program, args)
    } else {
        ("/bin/sleep".into(), vec!["30".into()])
    }
}

/// [`background_program`] as an app's command line.
pub fn background_command() -> String {
    let (program, args) = background_program();
    format!(r#""{program}" {}"#, args.join(" "))
}
