---
title: "Secret Storage"
description: "Where DBCrust keeps saved passwords, AI keys and tokens: the OS keychain or a passphrase-encrypted file, never a machine-derived key. How to inspect, audit and migrate."
---

# Secret Storage

DBCrust keeps every secret it saves — database passwords, AI API keys, OAuth tokens — in one **secret store**. The store is either your operating system's keychain or a file encrypted with a key that comes from *you* (a passphrase), never from your machine's identifiers.

```bash
dbcrust secrets status      # where secrets live, lock state, how many (never values)
dbcrust secrets audit       # scan for credentials left in files (never prints values)
dbcrust secrets migrate     # move legacy passwords / keys into the store
dbcrust secrets unlock      # Linux file backend: enter the passphrase once per login session
dbcrust secrets lock        # forget the cached unlock key
dbcrust secrets delete <id> # remove one stored secret
```

## Where secrets live

| Platform | Default (`backend = "auto"`) | What protects it |
|----------|------------------------------|------------------|
| **macOS** | Keychain (service `dbcrust.store`) | The keychain's per-application access list: another program reading the item triggers a prompt |
| **Windows** | Credential Manager (service `dbcrust.store`) | Your Windows account; other programs running as you can read it |
| **Linux and others** | `secrets.enc` in the DBCrust config directory | AES-256-GCM under a key derived from your passphrase with Argon2id |

Choose another backend with `dbcrust config set secrets.backend keychain|file|auto`. There is **no silent fallback**: if the chosen backend is locked or unavailable DBCrust says so and tells you what to run, instead of quietly using something weaker.

### The Linux passphrase file

`secrets.enc` is a single file in `~/.config/dbcrust/` (or `$DBCRUST_CONFIG_DIR`):

- Created with mode `0600`, written atomically (temp file, fsync, rename); symbolic links are refused.
- Each secret is its own AES-256-GCM record with a random nonce, bound to its id, so a ciphertext copied under another id does not open.
- The key comes from Argon2id (64 MiB, 3 passes; the parameters are stored in the file header). The passphrase is asked with a hidden prompt the first time you save a secret, and is **never** read from arguments, environment variables or disk.
- To avoid asking on every command, the derived key is cached in the Linux kernel *session keyring* (keyutils), which lasts until you log out. `dbcrust secrets lock` clears it; `secrets.file_unlock_cache = false` disables it. Where keyutils is unavailable nothing is cached.

If you forget the passphrase the file cannot be recovered: delete `secrets.enc` and enter your database passwords again (or let DBCrust prompt when it needs them). Your databases are unaffected.

### No prompts where there is no human

One-shot runs (`-c` / `-f`), `--no-input`, a piped stdin and the desktop app never prompt. If the store is locked they fail with `secret storage is locked … run "dbcrust secrets unlock"`. Run `dbcrust secrets unlock` once in a terminal and later non-interactive runs in the same login session work.

## The threat model, in plain words

- **What it protects against:** someone who copies your files (backups, a stolen disk, a leaked dotfiles repo) gets nothing useful; DBCrust no longer leaves plaintext or "obfuscated" passwords in `~/.dbcrust`.
- **Targeted use:** a stored password is bound to its exact target (engine, host, port, database, user, TLS mode). Pointing a saved session at another host does not hand it the password — DBCrust refuses (`TargetMismatch`) and asks again.
- **What it does not protect against:** other programs running as your user *while the store is unlocked*. On Linux and Windows there is no per-application access control; on macOS the keychain prompts for programs other than DBCrust, but a program that simply runs `dbcrust` can use your saved connections. If an AI agent runs as your user, treat anything DBCrust can reach as reachable by the agent. Use a least-privilege database role, and for connections you want agents to use, the [credential broker](/dbcrust/user-guide/agent-connections/), whose service mode keeps passwords under a different OS user.
- **Machine-derived keys are gone.** Earlier versions "encrypted" `~/.dbcrust` with a key derived from machine-id/hostname/username. Any process of yours could recompute it, so it protected nothing. DBCrust still *reads* those `enc:` values so you can migrate them, and never writes new ones.

## Migrating from `~/.dbcrust` and `ai_keys.enc`

```bash
dbcrust secrets audit            # what is lying around (exit 1 if anything high-severity)
dbcrust secrets migrate --dry-run
dbcrust secrets migrate
```

`migrate` is interactive: it lists the DBCrust-owned legacy secrets it found, lets you deselect items, and asks before changing anything. It handles:

- `~/.dbcrust` entries (plaintext and legacy `enc:`), including `$DBCRUST_PASSFILE`;
- `~/.config/dbcrust/ai_keys.enc` and the old `dbcrust` keychain entries for AI providers;
- secret options and SSH passwords still present in `sessions.toml`.

How it stays safe:

1. Each secret is written to the store, **read back and compared**, and only then is its legacy copy removed.
2. Legacy files are rewritten atomically and keep everything that was not migrated (comments, unreadable lines, entries you skipped). `ai_keys.enc` is deleted only once empty.
3. A small journal (`secrets_migration.toml`, ids and states only) means an interrupted run can simply be started again.
4. **Wildcard entries** (`*` in host/port/database/user) are never bound automatically — they would hand one password to every matching target. `migrate` lists them and lets you bind one to a concrete target or skip it. `--yes` (non-interactive) migrates only non-wildcard items.
5. **`.pgpass` and `.my.cnf` are never edited.** They are third-party files; `audit` reports them and their permissions.

Flags: `--dry-run` prints the plan and changes nothing; `--yes` skips the questions for non-wildcard items. The command is idempotent.

After a migration, until you migrate, a password found only in a legacy file is still *used* (read-only) and DBCrust prints a one-line hint on stderr suggesting `dbcrust secrets migrate`.

## What `audit` looks at

`dbcrust secrets audit [-o json]` reads only known locations, never follows symlinks, caps file sizes at 10 MiB and **never prints a matched value** — only rule id, path, line, severity and a remediation. It does not run password commands or decrypt anything.

| Rule family | Where | Severity |
|-------------|-------|----------|
| `legacy.dbcrust.*` | `~/.dbcrust`: plaintext lines, `enc:` machine-key lines, wildcards, unreadable lines, loose permissions | high / medium / low |
| `legacy.ai_keys.*`, `legacy.keychain.ai` | `ai_keys.enc`, old keychain entries (existence only) | high / medium / low |
| `storage.*` | `sessions.toml`, `recent.toml`: URLs with passwords, secret-looking options, SSH passwords | high |
| `config.ssh-tunnel-password` | `config.toml` `ssh_tunnel_patterns` values with `user:pass@` | high |
| `history.*`, `named-queries.*` | REPL history files, named queries: URLs with passwords, `PASSWORD '…'` literals | medium |
| `third-party.*` | `.pgpass`, `.my.cnf`, `~/.vault-token`: presence and permissions | info / high |
| `store.*` | unavailable store, index drift | medium / low |

**Exit code:** `0` when there are no high-severity findings, `1` otherwise — suitable for CI or a pre-commit check. `-o json` prints `{ok, summary, findings, scanned}`.

## Related files

| File | Holds | Secrets? |
|------|-------|----------|
| `secrets.enc` (file backend) | encrypted records | encrypted |
| `secrets_index.toml` | record ids, engine/host/port/database/user, timestamps | **never** |
| `secrets_migration.toml` | migration progress (only while a migration is unfinished) | **never** |
| `sessions.toml`, `recent.toml` | connection metadata | no secrets; secret-looking options move to the store |

`vault_credentials.enc` (the Vault credential cache) is a separate mechanism: it is encrypted with a key derived from your Vault token (not from the machine) and is only as strong as the protection of that token (`$VAULT_TOKEN` / `~/.vault-token`). Anyone who can read the token can read the cache.

## Configuration

```toml
[secrets]
backend = "auto"          # auto | keychain | file
file_unlock_cache = true  # keep the unlocked file key for the login session (Linux keyutils)
```

Both keys work with `dbcrust config get/set` and `\config`.
