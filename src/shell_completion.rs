use clap::Command;
use clap_complete::{Shell, generate};
use std::io::Write;

use crate::url_scheme::UrlScheme;
use strum::IntoEnumIterator;

/// Generate shell completion scripts with custom URL scheme support
pub fn generate_completion_with_url_schemes<W: Write>(
    shell: Shell,
    cmd: &mut Command,
    bin_name: &str,
    buf: &mut W,
) -> std::io::Result<()> {
    match shell {
        Shell::Bash => generate_bash_completion(cmd, bin_name, buf),
        Shell::Zsh => generate_zsh_completion(cmd, bin_name, buf),
        Shell::Fish => generate_fish_completion(cmd, bin_name, buf),
        _ => {
            // For other shells, fall back to standard clap_complete
            generate(shell, cmd, bin_name, buf);
            Ok(())
        }
    }
}

/// Generate Bash completion with URL scheme support
fn generate_bash_completion<W: Write>(
    cmd: &mut Command,
    bin_name: &str,
    buf: &mut W,
) -> std::io::Result<()> {
    // First, generate the standard completion
    let mut standard_completion = Vec::new();
    generate(Shell::Bash, cmd, bin_name, &mut standard_completion);

    // Convert to string for modification
    let mut completion_script = String::from_utf8_lossy(&standard_completion).into_owned();

    // Find the main completion function and add our custom URL completion logic with dynamic binary name
    let custom_functions = format!(
        r#"
# DBCrust URL scheme completion
_{bin_name}_complete_url_schemes() {{
    local schemes=(
"#
    );

    // Add all URL schemes
    let mut scheme_list = String::from(&custom_functions);
    for scheme in UrlScheme::iter() {
        scheme_list.push_str(&format!("        \"{}\"\n", scheme.url_prefix()));
    }
    scheme_list.push_str("    )\n    printf '%s\\n' \"${schemes[@]}\"\n}\n\n");

    // Function to complete docker containers
    scheme_list.push_str(&format!(r#"
_{bin_name}_complete_docker_containers() {{
    local containers
    if command -v docker &> /dev/null; then
        # Get running database containers
        containers=$(docker ps --format '{{{{.Names}}}}\t{{{{.Image}}}}' 2>/dev/null | grep -E 'postgres|mysql|mariadb|sqlite' | cut -f1 || true)
        if [[ -n "$containers" ]]; then
            printf '%s\n' $containers
        fi
    fi
}}

# Function to complete saved sessions
_{bin_name}_complete_sessions() {{
    local sessions
    local config_file="$HOME/.config/dbcrust/sessions.toml"
    if [[ -f "$config_file" ]]; then
        # Extract session names from TOML file
        sessions=$(grep '^\[sessions\.[^.]*\]$' "$config_file" 2>/dev/null | sed 's/\[sessions\.\(.*\)\]/\1/' || true)
        if [[ -n "$sessions" ]]; then
            printf '%s\n' $sessions
        fi
    fi
}}

# Custom URL completion
_{bin_name}_complete_url() {{
    local cur="$1"

    # If the current word doesn't contain "://", complete schemes
    if [[ "$cur" != *"://"* ]]; then
        _{bin_name}_complete_url_schemes | grep "^$cur"
    else
        # Extract the scheme and the part after "://"
        local scheme="${{cur%%://*}}"
        local after_scheme="${{cur#*://}}"

        case "$scheme" in
            docker)
                # Complete docker container names
                local containers=$(_{bin_name}_complete_docker_containers)
                if [[ -n "$containers" ]]; then
                    printf '%s\n' "$containers" | while read -r container; do
                        if [[ "$container" == "$after_scheme"* ]]; then
                            echo "docker://$container"
                        fi
                    done
                fi
                ;;
            session)
                # Complete session names
                local sessions=$(_{bin_name}_complete_sessions)
                if [[ -n "$sessions" ]]; then
                    printf '%s\n' "$sessions" | while read -r session; do
                        if [[ "$session" == "$after_scheme"* ]]; then
                            echo "session://$session"
                        fi
                    done
                fi
                ;;
            sqlite)
                # For SQLite, use file completion
                compopt -o default 2>/dev/null || true
                ;;
        esac
    fi
}}
"#));

    // Insert our custom functions before the main completion function
    let insertion_point = completion_script
        .find(&format!("_{bin_name}() {{"))
        .or_else(|| completion_script.find("_dbcrust() {"))
        .unwrap_or(0);
    completion_script.insert_str(insertion_point, &scheme_list);

    // Modify the completion function to use our custom URL completion
    // Find where positional arguments are handled and add our custom logic
    if let Some(pos) = completion_script.find("local context;") {
        let insert_code = format!(
            r#"
            # Check if we're completing the first positional argument (URL)
            if [[ ${{cur}} == -* ]] || [[ ${{argc}} -eq 0 ]]; then
                # If it's a flag or the first argument, check for URL completion
                if [[ ${{argc}} -eq 0 ]] || [[ ${{prev}} == "{bin_name}" ]]; then
                    local url_completions=$(_{bin_name}_complete_url "$cur")
                    if [[ -n "$url_completions" ]]; then
                        COMPREPLY+=( $(printf '%s\n' "$url_completions") )
                    fi
                fi
            fi
"#
        );
        completion_script.insert_str(pos, &insert_code);
    }

    buf.write_all(completion_script.as_bytes())?;
    Ok(())
}

/// Generate Zsh completion with URL scheme support
fn generate_zsh_completion<W: Write>(
    cmd: &mut Command,
    bin_name: &str,
    buf: &mut W,
) -> std::io::Result<()> {
    // First, generate the standard completion
    let mut standard_completion = Vec::new();
    generate(Shell::Zsh, cmd, bin_name, &mut standard_completion);

    // Convert to string for modification
    let mut completion_script = String::from_utf8_lossy(&standard_completion).into_owned();

    // Add custom URL completion functions with dynamic binary name
    let mut scheme_functions = format!(
        r#"
# DBCrust URL scheme completion functions
_{bin_name}_url_schemes() {{
    local schemes=(
"#
    );

    for scheme in UrlScheme::iter() {
        scheme_functions.push_str(&format!(
            "        '{}:{}'\n",
            scheme.url_prefix(),
            scheme.description()
        ));
    }
    scheme_functions.push_str(&format!(r#"    )
    _describe 'url schemes' schemes
}}

_{bin_name}_docker_containers() {{
    local containers
    if (( $+commands[docker] )); then
        containers=(${{(f)"$(docker ps --format '{{{{.Names}}}}\t{{{{.Image}}}}' 2>/dev/null | grep -E 'postgres|mysql|mariadb|sqlite' | cut -f1 || true)"}})
        if [[ -n "$containers" ]]; then
            _values 'docker containers' $containers
        fi
    fi
}}

_{bin_name}_sessions() {{
    local sessions
    local config_file="$HOME/.config/dbcrust/sessions.toml"
    if [[ -f "$config_file" ]]; then
        sessions=(${{(f)"$(grep '^\[sessions\.[^.]*\]$' "$config_file" 2>/dev/null | sed 's/\[sessions\.\(.*\)\]/\1/' || true)"}})
        if [[ -n "$sessions" ]]; then
            _values 'saved sessions' $sessions
        fi
    fi
}}

_{bin_name}_complete_url() {{
    local curcontext="$curcontext" state line
    local current_word="${{words[CURRENT]}}"

    if [[ "$current_word" != *"://"* ]]; then
        # Complete URL schemes - use compadd to add full scheme with ://
        local -a scheme_completions
        scheme_completions=(
            'postgres://'
            'mysql://'
            'sqlite://'
            'docker://'
            'session://'
            'recent://'
            'vault://'
        )
        compadd -S "" -a scheme_completions
    else
        # Complete based on scheme
        local scheme="${{current_word%%://*}}"
        case "$scheme" in
            docker)
                # For docker://, complete container names with docker:// prefix
                local containers
                if (( $+commands[docker] )); then
                    containers=(${{(f)"$(docker ps --format '{{{{.Names}}}}\t{{{{.Image}}}}' 2>/dev/null | grep -E 'postgres|mysql|mariadb|sqlite' | cut -f1 || true)"}})
                    if [[ -n "$containers" ]]; then
                        local -a docker_completions
                        for container in "$containers[@]"; do
                            docker_completions+=("docker://$container")
                        done
                        compadd -S "" -a docker_completions
                    fi
                fi
                ;;
            session)
                # For session://, complete session names with session:// prefix
                local sessions config_file="$HOME/.config/dbcrust/sessions.toml"
                if [[ -f "$config_file" ]]; then
                    sessions=(${{(f)"$(grep '^\[sessions\.[^.]*\]$' "$config_file" 2>/dev/null | sed 's/\[sessions\.\(.*\)\]/\1/' || true)"}})
                    if [[ -n "$sessions" ]]; then
                        local -a session_completions
                        for session in "$sessions[@]"; do
                            session_completions+=("session://$session")
                        done
                        compadd -S "" -a session_completions
                    fi
                fi
                ;;
            sqlite)
                _files -g "*.db *.sqlite *.sqlite3"
                ;;
        esac
    fi
}}
"#));

    // Insert custom functions after #compdef but before main function
    let insert_pos = if let Some(pos) = completion_script.find(&format!("_{bin_name}() {{")) {
        pos
    } else if let Some(pos) = completion_script
        .find("_dbc() {")
        .or_else(|| completion_script.find("_dbcrust() {"))
    {
        pos
    } else {
        // Fallback: insert after #compdef line
        completion_script.find('\n').map(|p| p + 1).unwrap_or(0)
    };
    completion_script.insert_str(insert_pos, &scheme_functions);

    // Modify the arguments section to include URL completion
    if let Some(pos) =
        completion_script.find("'::connection_url -- Database connection URL:_default'")
    {
        completion_script.replace_range(
            pos..pos + "'::connection_url -- Database connection URL:_default'".len(),
            &format!("'::connection_url -- Database connection URL:_{bin_name}_complete_url'"),
        );
    }

    buf.write_all(completion_script.as_bytes())?;
    Ok(())
}

/// Generate Fish completion with URL scheme support
fn generate_fish_completion<W: Write>(
    cmd: &mut Command,
    bin_name: &str,
    buf: &mut W,
) -> std::io::Result<()> {
    // First, generate the standard completion
    let mut standard_completion = Vec::new();
    generate(Shell::Fish, cmd, bin_name, &mut standard_completion);

    // Convert to string and add our custom completions
    let mut completion_script = String::from_utf8_lossy(&standard_completion).into_owned();

    // Add URL scheme completions
    let mut custom_completions = String::new();
    custom_completions.push_str("\n# DBCrust URL scheme completions\n");
    for scheme in UrlScheme::iter() {
        custom_completions.push_str(&format!(
            "complete -c {} -n '__fish_is_first_token; and not __fish_seen_subcommand_from {}' -a '{}' -d '{}'\n",
            bin_name,
            scheme.url_prefix(),
            scheme.url_prefix(),
            scheme.description()
        ));
    }

    // Add docker container completion
    custom_completions.push_str(&format!(r#"
# Docker container completion
function __dbcrust_docker_containers
    if command -q docker
        docker ps --format '{{{{.Names}}}}' | grep -E 'postgres|mysql|mariadb|sqlite' 2>/dev/null
    end
end

complete -c {bin_name} -n 'string match -q "docker://*" (commandline -ct)' -f -a '(printf "docker://%s\n" (__dbcrust_docker_containers))'
"#));

    // Add session completion
    custom_completions.push_str(&format!(r#"
# Session completion
function __dbcrust_sessions
    set -l config_file "$HOME/.config/dbcrust/sessions.toml"
    if test -f "$config_file"
        grep '^\[sessions\.[^.]*\]$' "$config_file" 2>/dev/null | sed 's/\[sessions\.\(.*\)\]/\1/'
    end
end

complete -c {bin_name} -n 'string match -q "session://*" (commandline -ct)' -f -a '(printf "session://%s\n" (__dbcrust_sessions))'
"#));

    completion_script.push_str(&custom_completions);

    buf.write_all(completion_script.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Command;

    #[test]
    fn test_bash_completion_generation() {
        let mut cmd = Command::new("dbcrust");
        let mut output = Vec::new();

        generate_completion_with_url_schemes(Shell::Bash, &mut cmd, "dbcrust", &mut output)
            .expect("Failed to generate bash completion");

        let script = String::from_utf8(output).expect("Invalid UTF-8");

        // Check that URL schemes are included
        assert!(script.contains("postgres://"));
        assert!(script.contains("docker://"));
        assert!(script.contains("session://"));

        // Check that custom functions are included
        assert!(script.contains("_dbcrust_complete_url_schemes"));
        assert!(script.contains("_dbcrust_complete_docker_containers"));
        assert!(script.contains("_dbcrust_complete_sessions"));
    }

    #[test]
    fn test_zsh_completion_generation() {
        let mut cmd = Command::new("dbcrust");
        let mut output = Vec::new();

        generate_completion_with_url_schemes(Shell::Zsh, &mut cmd, "dbcrust", &mut output)
            .expect("Failed to generate zsh completion");

        let script = String::from_utf8(output).expect("Invalid UTF-8");

        // Check that custom functions are included
        assert!(script.contains("_dbcrust_url_schemes"));
        assert!(script.contains("_dbcrust_docker_containers"));
        assert!(script.contains("_dbcrust_sessions"));
    }

    #[test]
    fn test_fish_completion_generation() {
        let mut cmd = Command::new("dbcrust");
        let mut output = Vec::new();

        generate_completion_with_url_schemes(Shell::Fish, &mut cmd, "dbcrust", &mut output)
            .expect("Failed to generate fish completion");

        let script = String::from_utf8(output).expect("Invalid UTF-8");

        // Check that URL schemes are included
        assert!(script.contains("postgres://"));
        assert!(script.contains("__dbcrust_docker_containers"));
        assert!(script.contains("__dbcrust_sessions"));
    }
}
