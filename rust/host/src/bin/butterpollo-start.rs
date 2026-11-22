#![windows_subsystem = "windows"]
fn main() {
    if let Err(error) = butterpollo_win_io::launcher::run() {
        butterpollo_win_io::launcher::show_error(&format!("{error:#}"));
    }
}
