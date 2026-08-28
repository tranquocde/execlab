mod config;
mod model;
mod results;
mod server;
mod store;
mod ui;

fn main() {
    if let Err(error) = config::load().and_then(server::serve) {
        eprintln!("trader_viz: {error}");
        std::process::exit(1);
    }
}
