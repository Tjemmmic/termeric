//! URL handling
//!
//! Provides functionality for opening URLs using the system handler.

/// URL handler for terminal content
pub struct UrlDetector;

impl UrlDetector {
    /// Open a URL using the system default handler
    pub fn open_url(url: &str) -> std::io::Result<()> {
        // Handle file:// URLs specially - extract the path portion
        let effective_url = if url.starts_with("file://") {
            Self::normalize_file_url(url)
        } else {
            url.to_string()
        };

        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open")
                .arg(&effective_url)
                .spawn()?;
        }

        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg(&effective_url)
                .spawn()?;
        }

        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd")
                .args(["/C", "start", "", &effective_url])
                .spawn()?;
        }

        Ok(())
    }

    /// Normalize a file:// URL to extract the local path
    ///
    /// Handles URLs like:
    /// - file:///path/to/file -> /path/to/file
    /// - file://localhost/path/to/file -> /path/to/file
    /// - file://hostname/path/to/file -> /path/to/file (for local files)
    fn normalize_file_url(url: &str) -> String {
        let without_scheme = url.strip_prefix("file://").unwrap_or(url);

        // Check if it starts with a path (three slashes total: file:///path)
        if without_scheme.starts_with('/') {
            // Already a proper local path: file:///path/to/file
            return without_scheme.to_string();
        }

        // Has a hostname: file://hostname/path/to/file
        // Find the first slash after the hostname
        if let Some(slash_pos) = without_scheme.find('/') {
            let hostname = &without_scheme[..slash_pos];
            let path = &without_scheme[slash_pos..];

            log::debug!(
                "Normalized file URL: hostname='{}', path='{}'",
                hostname,
                path
            );
            path.to_string()
        } else {
            // No path portion - malformed URL, return as-is
            format!("file://{}", without_scheme)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_file_url_with_hostname() {
        let result = UrlDetector::normalize_file_url("file://Emrakul/home/zilawon/Downloads");
        assert_eq!(result, "/home/zilawon/Downloads");
    }

    #[test]
    fn test_normalize_file_url_with_localhost() {
        let result = UrlDetector::normalize_file_url("file://localhost/home/user/file.txt");
        assert_eq!(result, "/home/user/file.txt");
    }

    #[test]
    fn test_normalize_file_url_triple_slash() {
        let result = UrlDetector::normalize_file_url("file:///home/user/file.txt");
        assert_eq!(result, "/home/user/file.txt");
    }
}
