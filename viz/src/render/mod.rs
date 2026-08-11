pub mod asset_comparison;
pub mod html;

use crate::process::Report;

pub trait Renderer {
    fn render(&self, report: &Report) -> Result<String, String>;
}
