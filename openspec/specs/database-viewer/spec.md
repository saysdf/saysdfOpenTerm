# database-viewer Specification

## Purpose
Browse databases from inside the terminal: saved connections, schema browser, table data, query editor with history, Dolt branches and result export. Passwords are never stored in connection files and network connections are encrypted.

## Requirements

### Requirement: Saved connections without passwords
The `saysdf db` utility SHALL work with saved connections to SQLite, PostgreSQL, MySQL and Dolt (name, description, type, host, port, database, user, SQLite file path, TLS mode) stored in the saysdfTerm config. Passwords MUST NOT be written to the config, the query history or logs; they SHALL be stored via `secret-storage` or entered at connection time. The utility MUST work on Windows, Linux and macOS.

#### Scenario: Connection list
- **WHEN** the user runs `saysdf db`
- **THEN** a list of saved connections is shown with name and description

#### Scenario: Password does not end up in the config
- **WHEN** the user saves a PostgreSQL connection with a password
- **THEN** the config file contains no password, and the password is kept in the system credential store

#### Scenario: Direct launch on a file
- **WHEN** the user runs `saysdf db path/to/app.sqlite`
- **THEN** the file is opened read-only without saving a connection

### Requirement: Encrypted network connections
For PostgreSQL, MySQL and Dolt the utility SHALL by default require TLS with certificate verification. Disabling TLS or verification MUST be an explicit connection setting and MUST be shown in the status line.

#### Scenario: Server without TLS
- **WHEN** the server does not support TLS and the connection uses the default mode
- **THEN** the connection is not established, and the message explains how to explicitly allow an unencrypted connection

#### Scenario: TLS explicitly disabled
- **WHEN** the connection has `tls = "disable"`
- **THEN** the connection works, and the status line shows an "unencrypted" mark

### Requirement: Schema browser
The utility SHALL show a tree of databases → schemas → tables and views, and for the selected table its columns (type, NULL, default value), indexes and foreign keys.

#### Scenario: Table structure
- **WHEN** the user selects a table in the tree
- **THEN** its columns, indexes and foreign keys are shown on the right

### Requirement: Table data view
The utility SHALL show table data page by page (LIMIT/OFFSET or keyset), with sorting by column, a filter (a parameterized WHERE condition or a text filter) and horizontal scrolling. Values from the database MUST be displayed with control characters escaped.

#### Scenario: Pagination
- **WHEN** the table has more rows than fit on a page
- **THEN** the first page is shown, and moving to the next one loads the next chunk

#### Scenario: Control characters in data
- **WHEN** a cell contains an ESC sequence
- **THEN** it is shown as visible characters and does not affect the terminal

### Requirement: Query editor with history
The utility SHALL provide a query editor with SQL highlighting and history, execute one statement at a time, and show the result as a table with the row count and elapsed time. The result MUST be limited by a configurable in-memory row limit.

#### Scenario: Running a SELECT
- **WHEN** the user enters `SELECT 1` and runs it
- **THEN** a result with one row and the execution time is shown

#### Scenario: Multiple statements
- **WHEN** the query text contains several statements separated by `;`
- **THEN** only the statement under the cursor is executed, or an error is shown, without running the others

### Requirement: Dolt branches and history
For Dolt connections the utility SHALL show branches (`dolt_branches`) and commit history (`dolt_log`), and allow viewing a table on a selected branch or commit via `AS OF` in read-only mode.

#### Scenario: Table at a commit
- **WHEN** the user selects a commit in the history and a table
- **THEN** the data is shown using the query `SELECT ... AS OF '<commit>'`

### Requirement: Export results
The utility SHALL export to CSV (RFC 4180) and JSON (array of objects), to a file chosen by the user, the whole table (with the current sorting and filter) or all rows of a query result, streaming rather than only the displayed page; an existing file MUST NOT be overwritten without confirmation. Re-running a query for export MUST be read-only.

#### Scenario: CSV export
- **WHEN** the user exports a result with commas and quotes in the values
- **THEN** the CSV file is correctly escaped and opens in a spreadsheet editor

#### Scenario: Export of the whole table
- **WHEN** the user exports from the grid a table with more rows than fit on one page
- **THEN** all rows of the table are written to the file, not just the current page
