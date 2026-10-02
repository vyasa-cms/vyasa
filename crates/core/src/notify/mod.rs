//! Transactional email: templated, queued, provider-flexible.

mod email;

pub use email::{render_template, EmailService, RenderedEmail};
