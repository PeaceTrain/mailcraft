# Grommunio groupware implementation

Target: native Windows, macOS and Linux clients for the latest stable grommunio.
Full scope includes mail, calendars, invitations, contacts, tasks, notes and shared resources.
EWS is the primary protocol under evaluation. A folder probe is not proof of item sync support.

## Milestones and acceptance gates

1. **Connection prototype (in progress):** read-only EWS distinguished-folder probe for Inbox,
   Calendar, Contacts, Tasks, Notes and mailbox root, with synthetic protocol tests. Basic over TLS
   is the initial authentication scope; NTLM, OAuth and Autodiscover remain pending.
2. **Server compatibility:** use a dedicated synthetic grommunio instance. Record exact grommunio
   and Gromox package versions, authentication configuration and endpoint. Verify folder and item
   read/write operations for each item type, synchronization tokens, shared permissions and errors.
   The latest stable release is the target, but no version is certified until this matrix is run.
3. **Persistent sync:** account setup, secure credentials, remote IDs/change keys, checkpoints,
   offline mutation queue, retries and conflict handling. Restart/disconnect tests must prove
   no lost changes or duplicate submissions. Never enable a placeholder Exchange account as working.
4. **Mail:** folders, MIME/attachments, drafts, sending, flags, moves and deletion; verify both
   directions with the synthetic server and grommunio Web.
5. **Personal groupware:** calendars including recurrence exceptions/time zones and invitations,
   contacts/groups, tasks/completion and notes. Verify creation, edits and deletion from either client.
6. **Shared resources:** shared mailboxes/calendars, delegation, directory lookup, availability and
   rooms, gated by tested server capabilities. Permission failures must remain explicit.
7. **Release:** large datasets, hostile responses, expired authentication, migrations, native
   keychains and installation packages on all three operating systems. Preserve upstream CI gates.

## Architecture and work allocation

Protocol code belongs in `crates/net`; account data and remote identities in `crates/model`;
storage migrations in `crates/store`; synchronization and commands in `crates/engine`.
The existing egui interface must dispatch commands rather than own network logic.
Keep the browser build compiling; native EWS support does not imply browser connectivity.

Use GPT-6 Luna for bounded coding tasks with explicit acceptance tests and GPT-6 Sol for review.
Fix review findings and rerun the relevant checks before considering a milestone complete.
Do not copy server implementation code or use real mail accounts for tests.

## Current limitations

The first increment is a library-level folder probe and synthetic diagnostic tooling only.
No live grommunio release has been validated. Account UI, item synchronization, offline groupware
operations and shared-resource workflows remain unimplemented. A successful folder response only
confirms folder visibility, not read/write permissions or support for later EWS operations.

## Running the diagnostic

Against a dedicated synthetic test mailbox, set `EWS_URL`, `EWS_USERNAME` and `EWS_PASSWORD`
in the process environment, then run `cargo run -p mailcraft-net --example ews_probe`.
The endpoint is supplied explicitly; Autodiscover is not implemented yet. Use HTTPS with a
trusted certificate. `EWS_ALLOW_PLAINTEXT=1` exists only for explicitly trusted HTTP test servers.
The diagnostic prints folder availability/errors without printing supplied credentials.
It performs GetFolder only and does not modify the server.

Run `cargo test --locked -p mailcraft-net --all-targets` for the network regression suite.
The `net-platforms` workflow runs on Linux, macOS and Windows. A workflow definition alone is
not evidence that the three platforms passed; check the actual run before releasing.
