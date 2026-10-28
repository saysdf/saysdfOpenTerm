# ssh-connections Specification

## Purpose
Offer a searchable list of SSH connections taken from the user's ssh config and open the chosen one in a new tab. New entries are added without rewriting existing ones, and passwords and host keys are left to ssh.

## Requirements

### Requirement: Connection list from ssh config
The module SHALL build a list of connections from `~/.ssh/config`, expanding `Include` (via the `wezterm-ssh` parser), taking only literal, non-negated `Host` entries, and enrich them with saysdfTerm metadata (name, description, group, color) keyed by the host alias. The module MUST NOT modify `~/.ssh/config` or included files when reading. The module MUST work on Windows, Linux, and macOS.

#### Scenario: Hosts from Include
- **WHEN** `~/.ssh/config` contains `Include config.d/*` and `Host build` is defined in `config.d/work`
- **THEN** `build` appears in the connection list

#### Scenario: Patterns are excluded from the list
- **WHEN** the config contains `Host *`, `Host *.corp`, and `Host !bad`
- **THEN** these entries are not shown as connections

#### Scenario: Metadata
- **WHEN** the module settings define the name "Build server" and a description for the host `build`
- **THEN** the list shows the name, and the description is available on selection (a second column or a hint line)

#### Scenario: No ssh config
- **WHEN** the file `~/.ssh/config` does not exist
- **THEN** the list is empty and a hint on how to add a connection is shown, with no error

### Requirement: Connection picker with search
The module SHALL show a connection picker with fuzzy search over alias, name, description, and group; the description MUST be visible without a separate tooltip (a second column of the picker row).

#### Scenario: Search by description
- **WHEN** the user opens the picker and types a word from a host's description
- **THEN** the list keeps the hosts that have this word in their name, description, group, or alias

### Requirement: Open connection in new tab
The module SHALL open the selected connection in a new tab (or a split, as chosen) by launching the system `ssh <alias>`, coloring the tab with the connection's color if one is set. If the system `ssh` is not found, the module MUST show a notification and MAY offer the built-in ssh domain.

#### Scenario: Open
- **WHEN** the user selects the connection `build`
- **THEN** a new tab with `ssh build` opens, and its title is the connection's name

#### Scenario: No ssh
- **WHEN** `ssh` is not in `PATH`
- **THEN** no tab is created and a notification says what is missing

### Requirement: Add connection without rewriting user entries
The module SHALL add a new connection through a form (alias, HostName, User, Port, IdentityFile, name, description, group) by writing a `Host` block to a separate saysdfTerm file (`~/.ssh/saysdfterm.conf`), and the metadata to the saysdfTerm metadata file. The module MUST NOT modify the user's existing entries. The module MUST add an `Include` line at the top of `~/.ssh/config` only after explicit user confirmation, with a backup, and MUST NOT add it again.

#### Scenario: Write a new host
- **WHEN** the user saves the form with the alias `new`
- **THEN** a `Host new` block is added to `saysdfterm.conf`, and the contents of `~/.ssh/config` and other files, apart from the possible `Include` line, are byte-for-byte unchanged

#### Scenario: Include confirmation
- **WHEN** `~/.ssh/config` does not yet include `saysdfterm.conf`
- **THEN** the module asks for confirmation; if the user declines, the host is saved to the file, but the module reports that `ssh new` will not see it without `Include`

#### Scenario: Duplicate alias
- **WHEN** the alias is already in the host list
- **THEN** the form shows an error and writes nothing

#### Scenario: Invalid values
- **WHEN** a field contains a newline, a space in the alias, or the pattern characters `*?!`
- **THEN** the form rejects the value

### Requirement: Secrets and host keys
The module MUST NOT store passwords and passphrases in the config, metadata, or ssh config; authentication SHALL go through ssh-agent, keys, or interactive input of the system `ssh`. Host key verification SHALL remain with the system `ssh` (`known_hosts`); the module MUST NOT disable `StrictHostKeyChecking`. The file `saysdfterm.conf` SHALL be created with permissions for the user only.

#### Scenario: First connection
- **WHEN** the user connects to a host for the first time
- **THEN** the system `ssh` in the tab shows the key fingerprint and asks for confirmation
