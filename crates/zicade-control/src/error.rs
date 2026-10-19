//! Why an edited config was not applied.

use zicade_config::ConfigError;

/// An apply that changed nothing; the message is meant for the user.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// The edited config failed validation.
    #[error("{0}")]
    Invalid(#[source] ConfigError),
    /// The config is valid but could not be written to disk.
    #[error("could not save the configuration: {0}")]
    Save(#[source] ConfigError),
}
