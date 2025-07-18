use std::io::{ErrorKind, Write};
use std::process::{Command, Stdio};

#[allow(dead_code)]
pub fn page_output(content: &str, pager_cmd_str: &str) -> std::io::Result<()> {
    if content.is_empty() {
        return Ok(());
    }

    let parts: Vec<&str> = pager_cmd_str.split_whitespace().collect();
    if parts.is_empty() {
        // No pager command, print directly (should not happen if correctly configured)
        // but as a fallback, we print.
        print!("{}", content);
        return Ok(());
    }

    let cmd_name = parts[0];
    let cmd_args = &parts[1..];

    let mut command_process = Command::new(cmd_name);
    command_process.args(cmd_args);

    let child = command_process
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit()) // Pager should take over the terminal
        .stderr(Stdio::inherit())
        .spawn();

    match child {
        Ok(mut child_process) => {
            if let Some(mut stdin) = child_process.stdin.take() {
                if let Err(e) = stdin.write_all(content.as_bytes()) {
                    if e.kind() != ErrorKind::BrokenPipe {
                        // BrokenPipe is expected if `less` exits early (e.g. small content, or user quits)
                        // For other errors, print them and fallback.
                        eprintln!("Error writing to pager stdin: {}", e);
                        print!("{}", content); // Fallback to direct print
                        return Err(e);
                    }
                }
            } // stdin is dropped here, signaling EOF to the pager

            // Wait for the pager process to complete
            match child_process.wait() {
                Ok(status) => {
                    if !status.success() {
                        // Pager exited with non-zero status, but might have still displayed content.
                        // Log this, but don't necessarily fallback to direct print unless it's critical.
                        // e.g. less might return non-zero if file not found, but we pipe to stdin.
                        // For now, we assume if wait was Ok, pager handled it or user quit.
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Pager process exited with an error: {}", e);
                    print!("{}", content); // Fallback to direct print
                    Err(e)
                }
            }
        }
        Err(e) => {
            eprintln!(
                "Failed to start pager '{}': {}. Outputting directly.",
                pager_cmd_str, e
            );
            print!("{}", content); // Fallback to direct print
            Err(e)
        }
    }
}
