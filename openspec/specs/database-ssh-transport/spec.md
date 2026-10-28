# database-ssh-transport Specification

## Purpose
Let the database viewer reach a remote PostgreSQL server over SSH without open ports or manual tunnels. The remote command is injection-safe and the session is read-only by default.

## Requirements

### Requirement: PostgreSQL over SSH without ports or tunnels
`saysdf db` SHALL support the connection type `postgres-ssh` with the fields: ssh alias, container
(optional; empty means psql on the host), database, user. The connection SHALL be a single
long-lived `ssh` process (arguments passed as a list, no local shell) with `BatchMode=yes`;
the database port and tunnels MUST NOT be used. The password MUST NOT be passed on the command line.

#### Scenario: Query via ssh and docker exec
- **WHEN** the user runs `saysdf db --conn prod --query "select 1" --print` for a `postgres-ssh` connection
- **THEN** the query is executed via `ssh <alias> docker exec -i <container> psql …` and the result is printed as CSV

#### Scenario: Without docker
- **WHEN** the "Container" field is empty
- **THEN** psql is launched directly on the host

### Requirement: Injection-safe remote command
The alias, container, database and user values SHALL be validated against `[A-Za-z0-9_.-]` and MUST NOT
start with `-`; otherwise the connection MUST be rejected before any process is started. The user's
query text MUST NOT contain a backslash (psql meta-commands), unclosed quotes
or comments; `COPY` is not supported through this connection type.

#### Scenario: Injection attempt
- **WHEN** the container is set to `db; rm -rf /` or `$(id)`
- **THEN** the connection is rejected with a clear error and no `ssh` process is started

#### Scenario: psql meta-command
- **WHEN** the query contains `\!`
- **THEN** the query is rejected and nothing is sent to the server

### Requirement: Read-only by default over SSH
Immediately after startup the session SHALL run `SET default_transaction_read_only = on`; every query
in read-only mode SHALL be executed within `BEGIN READ ONLY` … `ROLLBACK`. Dangerous mode
(`WriteMode`) SHALL lift read-only only at the session level; export MUST remain read-only.

#### Scenario: Read-only session
- **WHEN** a connection is opened without dangerous mode
- **THEN** `current_setting('default_transaction_read_only')` equals `on`

### Requirement: Framed protocol, errors and reconnection
Responses SHALL be separated by markers carrying a random session token, and data SHALL be CSV (multiline
values and quotes are supported, and NULL is distinguishable from an empty string). A psql error SHALL be returned as text
with its SQLSTATE, and the session keeps running. A timeout or a dropped connection SHALL reset the connection; the next
query reconnects; a query MUST be retried automatically only in read-only mode.

#### Scenario: SQL error
- **WHEN** a query refers to a table that does not exist
- **THEN** the psql error is shown with its SQLSTATE and the next query runs normally
