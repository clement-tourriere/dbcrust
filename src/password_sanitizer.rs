use regex::Regex;
use url::Url;

/// Sanitize a connection URL by removing the password
pub fn sanitize_connection_url(url: &str) -> String {
    // Check if this is our special Docker format with comment
    if let Some(comment_pos) = url.find(" # Docker: ") {
        let main_url = &url[..comment_pos];
        let comment_part = &url[comment_pos..];
        
        // Sanitize the main URL part and re-add the comment
        let sanitized_main = sanitize_connection_url(main_url);
        return format!("{}{}", sanitized_main, comment_part);
    }
    
    // First check if this looks like a connection string without a proper scheme
    if !url.starts_with("postgresql://")
        && !url.starts_with("postgres://")
        && url.contains('@')
        && url.contains(':')
    {
        // This looks like a simple connection string format: user:password@host:port/db
        let parts: Vec<&str> = url.split('@').collect();
        if parts.len() >= 2 {
            let user_pass = parts[0];
            let rest = parts[1..].join("@");
            if user_pass.contains(':') {
                let user_parts: Vec<&str> = user_pass.splitn(2, ':').collect();
                if user_parts.len() == 2 {
                    return format!("{}:[REDACTED]@{}", user_parts[0], rest);
                }
            }
        }
    }

    // Try to parse as a proper URL
    if let Ok(parsed) = Url::parse(url) {
        let mut sanitized = parsed.clone();
        if parsed.password().is_some() {
            let _ = sanitized.set_password(Some("[REDACTED]"));
        }
        // URL-decode the final result to make [REDACTED] readable in logs
        let result = sanitized.to_string();
        result.replace("%5BREDACTED%5D", "[REDACTED]")
    } else {
        // If URL parsing fails, return the original
        url.to_string()
    }
}

/// Sanitize an SSH tunnel string by removing the password
pub fn sanitize_ssh_tunnel_string(tunnel: &str) -> String {
    // Format: [user[:password]@]ssh_host[:ssh_port]
    if tunnel.contains('@') && tunnel.contains(':') {
        let parts: Vec<&str> = tunnel.split('@').collect();
        if parts.len() >= 2 {
            let user_pass = parts[0];
            let rest = parts[1..].join("@");
            if user_pass.contains(':') {
                let user_parts: Vec<&str> = user_pass.split(':').collect();
                if user_parts.len() >= 2 {
                    return format!("{}:[REDACTED]@{}", user_parts[0], rest);
                }
            }
        }
    }
    tunnel.to_string()
}

/// Sanitize SSH command strings that might contain passwords or sensitive information
#[allow(dead_code)]
pub fn sanitize_ssh_command(command: &str) -> String {
    // Look for patterns like -o PasswordAuthentication=yes password or similar
    // This is a basic implementation - SSH commands can be complex
    let mut sanitized = command.to_string();

    // Replace any obvious password patterns
    if sanitized.contains("password") || sanitized.contains("Password") {
        // This is a simple approach - in practice SSH passwords are usually not in command line
        // but could be in environment variables or other mechanisms
        sanitized = "[SSH_COMMAND_WITH_POTENTIAL_CREDENTIALS_REDACTED]".to_string();
    }

    sanitized
}

/// General sanitization function for any text that might contain passwords in various formats.
/// This uses regex patterns to match common password patterns and redact them.
#[allow(dead_code)]
pub fn sanitize_text_for_logging(text: &str) -> String {
    let mut sanitized = text.to_string();

    // Look for PostgreSQL connection URLs first (more specific)
    if sanitized.contains("postgresql://") || sanitized.contains("postgres://") {
        let re = Regex::new(r"postgres(?:ql)?://([^:]+):([^@]+)@([^/\s]+)").unwrap();
        sanitized = re
            .replace_all(&sanitized, "postgresql://$1:[REDACTED]@$3")
            .to_string();
    } else {
        // Only look for simple connection strings if no PostgreSQL URL was found
        let re = Regex::new(r"([a-zA-Z0-9_]+):([^@\s]+)@([a-zA-Z0-9.-]+)").unwrap();
        sanitized = re.replace_all(&sanitized, "$1:[REDACTED]@$3").to_string();
    }

    // Look for password= patterns (this can run regardless)
    let re = Regex::new(r"password=([^\s&]+)").unwrap();
    sanitized = re
        .replace_all(&sanitized, "password=[REDACTED]")
        .to_string();

    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_docker_connection_url() {
        // Test Docker URL with comment and password
        let docker_url = "postgresql://user:password@host:5432/db # Docker: container-name";
        let sanitized = sanitize_connection_url(docker_url);
        assert_eq!(sanitized, "postgresql://user:[REDACTED]@host:5432/db # Docker: container-name");
        
        // Test Docker URL without password
        let docker_url_no_pass = "postgresql://user@host:5432/db # Docker: container-name";
        let sanitized_no_pass = sanitize_connection_url(docker_url_no_pass);
        assert_eq!(sanitized_no_pass, "postgresql://user@host:5432/db # Docker: container-name");
    }

    #[test]
    fn test_sanitize_connection_url_with_password() {
        let url = "postgresql://user:secret123@localhost:5432/mydb";
        let sanitized = sanitize_connection_url(url);
        // Should now contain [REDACTED] without URL encoding
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains("secret123"));
        assert!(!sanitized.contains("%5BREDACTED%5D")); // Should not be URL encoded
        assert!(sanitized.contains("user"));
        assert!(sanitized.contains("localhost"));
    }

    #[test]
    fn test_sanitize_connection_url_without_password() {
        let url = "postgresql://user@localhost:5432/mydb";
        let sanitized = sanitize_connection_url(url);
        assert_eq!(sanitized, url);
    }

    #[test]
    fn test_sanitize_connection_url_simple_format() {
        let url = "user:password@host:5432/db";
        let sanitized = sanitize_connection_url(url);
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains("password"));
        assert!(sanitized.contains("user"));
    }

    #[test]
    fn test_sanitize_ssh_tunnel_string_with_password() {
        let tunnel = "user:secret@jumphost.com:22";
        let sanitized = sanitize_ssh_tunnel_string(tunnel);
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains("secret"));
        assert!(sanitized.contains("user"));
        assert!(sanitized.contains("jumphost.com"));
    }

    #[test]
    fn test_sanitize_ssh_tunnel_string_without_password() {
        let tunnel = "user@jumphost.com:22";
        let sanitized = sanitize_ssh_tunnel_string(tunnel);
        assert_eq!(sanitized, tunnel);
    }

    #[test]
    fn test_sanitize_ssh_command() {
        let command = "ssh -L 5432:localhost:5432 user@host";
        let sanitized = sanitize_ssh_command(command);
        assert_eq!(sanitized, command); // No password in this command

        let command_with_password = "ssh -o PasswordAuthentication=yes user@host";
        let sanitized_with_password = sanitize_ssh_command(command_with_password);
        assert!(
            sanitized_with_password.contains("[SSH_COMMAND_WITH_POTENTIAL_CREDENTIALS_REDACTED]")
        );
    }

    #[test]
    fn test_sanitize_text_for_logging() {
        // Test PostgreSQL URL sanitization
        let text = "Error connecting to postgresql://user:secret@localhost:5432/db";
        let sanitized = sanitize_text_for_logging(text);
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains("secret"));
        assert!(sanitized.contains("user"));

        // Test simple connection string sanitization
        let text = "Connection failed for user:password@host.com";
        let sanitized = sanitize_text_for_logging(text);
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains("password"));
        assert!(sanitized.contains("user"));

        // Test password= pattern sanitization
        let text = "Connection string: host=localhost password=secret123 user=test";
        let sanitized = sanitize_text_for_logging(text);
        assert!(sanitized.contains("password=[REDACTED]"));
        assert!(!sanitized.contains("secret123"));
        assert!(sanitized.contains("user=test"));

        // Test text without passwords
        let text = "Normal log message without sensitive data";
        let sanitized = sanitize_text_for_logging(text);
        assert_eq!(sanitized, text);
    }
}
