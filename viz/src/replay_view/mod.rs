//! Session replay view selection. Chart rendering can be added without
//! changing replay discovery or server routing.

mod display_simple;

pub enum DisplayMode {
    Simple,
    #[allow(dead_code)]
    Charts,
}

pub fn replay(mode: DisplayMode) -> &'static str {
    match mode {
        DisplayMode::Simple => display_simple::display_simple(),
        DisplayMode::Charts => display_charts(),
    }
}

fn display_charts() -> &'static str {
    // Plug the chart renderer in here later.
    display_simple::display_simple()
}
