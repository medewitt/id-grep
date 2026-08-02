pub mod abstracts;
pub mod config;
pub mod db;
pub mod output;
pub mod query;
pub mod sources;

mod error;
mod model;

pub use error::{Error, Result};
pub use model::Paper;
