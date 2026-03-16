/// sets the current working directory for the terminal
pub fn set_osc7(dir: &std::path::Path) {
    // TODO check the format string
    let osc = format!("\x1b]7;file://localhost{}\x07", &dir.to_string_lossy());
    print!("{}", osc);
}
