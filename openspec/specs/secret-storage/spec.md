# secret-storage Specification

## Purpose
Store secrets such as database passwords in the system keychain instead of in configuration files. When no keychain is available it degrades safely, and modules share one key namespace.

## Requirements

### Requirement: System keychain storage
saysdfTerm utilities SHALL store secrets (database passwords and, where needed, SSH passphrases) only in the system credential store: Windows Credential Manager, macOS Keychain, or Linux Secret Service. Secrets MUST NOT be written to config files, history, logs, or temporary files.

#### Scenario: Save a password
- **WHEN** the user enables "remember password" for a connection
- **THEN** the password is saved in the system store under the service `saysdfTerm` and the connection key

#### Scenario: Delete a connection
- **WHEN** the user deletes a saved connection
- **THEN** the associated secret is removed from the store

### Requirement: Graceful fallback without keychain
If the system store is unavailable (no Secret Service, store locked), the utility SHALL ask for the secret on every connection and MUST NOT save it in plain text.

#### Scenario: No Secret Service on Linux
- **WHEN** Secret Service is unavailable
- **THEN** the utility reports this and asks for the password on connect without saving it

### Requirement: Shared key namespace
Secret keys SHALL have the form `<domain>:<identifier>` (`db:<connection id>`, `ssh:<alias>`), so that the store is shared between `database-viewer` and `ssh-connections` without conflicts.

#### Scenario: Different domains
- **WHEN** a database connection `prod` and an SSH host `prod` both exist
- **THEN** their secrets are stored under different keys, `db:prod` and `ssh:prod`
