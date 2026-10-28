# database-write-mode Specification

## Purpose
Keep database access read-only by default and make writing an explicit, visible, temporary state. Dangerous mode is entered only through a dedicated button, shows a persistent banner, and resets automatically.

## Requirements

### Requirement: Read-only by default on the database side
Every connection SHALL be opened in a read-only mode enforced on the database side: SQLite — the `SQLITE_OPEN_READ_ONLY` flag and `PRAGMA query_only = ON`; PostgreSQL — `default_transaction_read_only = on` in the session parameters and every user query executed within `BEGIN READ ONLY` … `ROLLBACK`; MySQL and Dolt — `SET SESSION TRANSACTION READ ONLY` and `START TRANSACTION READ ONLY` … `ROLLBACK` for every query. A check in the interface alone MUST NOT be considered sufficient.

#### Scenario: Write to read-only SQLite
- **WHEN** an `INSERT` into a SQLite table is executed in read-only mode
- **THEN** the database returns a `readonly` error and the data in the file is unchanged

#### Scenario: Attempt to lift read-only from a query
- **WHEN** in read-only mode the user runs `SET SESSION TRANSACTION READ WRITE` or `PRAGMA query_only = OFF`
- **THEN** the statement is rejected and the mode remains read-only

### Requirement: Dangerous mode only via a dedicated button
Write mode ("DANGEROUS MODE") SHALL be enabled only by a dedicated interface button (a mouse click, or focus on the button and Enter) with a confirmation that states the database name. Enabling MUST NOT happen via a single hotkey, from the query text, from command-line arguments or from the config.

#### Scenario: Enabling
- **WHEN** the user presses the "Allow writes" button and confirms
- **THEN** the session reconnects in write mode and the dangerous-mode indication is turned on

#### Scenario: Confirmation cancelled
- **WHEN** the user cancels the confirmation dialog
- **THEN** the mode remains read-only

### Requirement: Persistent dangerous mode banner
While write mode is enabled, the first line of the panel SHALL permanently show a large red banner "DANGEROUS MODE (writes to <database> are allowed)", with the reason and the time it was enabled in parentheses or on hover. The banner MUST NOT be hidden by scrolling, dialogs or switching tabs inside the utility.

#### Scenario: Banner is always visible
- **WHEN** write mode is enabled and the user scrolls through data or opens the editor
- **THEN** the first line still shows the red banner with the database name

### Requirement: Single-action exit and automatic reset
Leaving write mode SHALL take a single action (a button on the banner or one key) without confirmation. Write mode MUST be reset to read-only on reconnection, on switching connection, on switching database or branch, and on restarting the utility.

#### Scenario: Exit
- **WHEN** the user presses the exit button on the red banner
- **THEN** the session returns to read-only mode and the banner disappears

#### Scenario: Switching connection
- **WHEN** in write mode the user switches to another connection
- **THEN** the new connection is opened in read-only mode
