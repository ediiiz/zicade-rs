//! Local UI token provisioning.

use std::io;
use std::path::Path;

use rand::Rng;

/// Number of hex characters in a generated token (128 bits of entropy).
const TOKEN_HEX_LEN: usize = 32;

/// Load the persisted UI token, or create one on first run.
///
/// Reads `%LOCALAPPDATA%\Zicade\ui-token` if present and non-empty; otherwise
/// generates a random 32-hex-char token, creates the directory, writes the
/// file, and returns the token. The file is written with the process's default
/// permissions (user-owned under the per-user LOCALAPPDATA tree).
pub fn load_or_create_token() -> io::Result<String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not set"))?;
    let dir = Path::new(&base).join("Zicade");
    let file = dir.join("ui-token");

    if let Ok(existing) = std::fs::read_to_string(&file) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_owned());
        }
    }

    std::fs::create_dir_all(&dir)?;
    let token = generate_token();
    std::fs::write(&file, &token)?;
    Ok(token)
}

/// Generate a random lowercase-hex token. Infallible (no `unwrap`/`expect`):
/// `from_digit` cannot fail for nibble values, but we fall back defensively.
fn generate_token() -> String {
    let mut rng = rand::rng();
    (0..TOKEN_HEX_LEN)
        .map(|_| {
            let nibble: u32 = rng.random_range(0..16);
            char::from_digit(nibble, 16).unwrap_or('0')
        })
        .collect()
}
